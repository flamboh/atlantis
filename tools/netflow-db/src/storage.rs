//! Concrete SQLite persistence and atomic database publication.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, backup::Backup,
    params,
};
use serde::Serialize;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tempfile::Builder;
use thiserror::Error;

use crate::provenance::{
    FileSnapshot, InputRevision, ProvenanceError, canonical_json, fingerprint,
};

pub const BUSY_TIMEOUT_MS: u64 = 60_000;
pub const STATS_TABLE_NAMES: [&str; 5] = [
    "traffic_stats",
    "protocol_stats",
    "address_count_stats",
    "port_count_stats",
    "address_structure_stats",
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Provenance(#[from] ProvenanceError),
    #[error("Cannot start {operation}: {active_operation} is active for {database}")]
    DatabaseOperationLocked {
        operation: String,
        active_operation: String,
        database: PathBuf,
    },
    #[error("Input revision mismatch for {locator:?}: {components} changed.")]
    InputRevisionConflict { locator: String, components: String },
    #[error("Pipeline product identity mismatch in {components}. {details}")]
    ProductIdentityConflict { components: String, details: String },
    #[error("{0}")]
    SourceLayoutConflict(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error("Database not found: {0}")]
    DatabaseNotFound(PathBuf),
}

/// A nonblocking process-lifetime lock for a database-wide operation.
#[derive(Debug)]
pub struct DatabaseOperationLock {
    file: File,
    path: PathBuf,
}

impl DatabaseOperationLock {
    pub fn acquire(
        database_path: impl AsRef<Path>,
        operation: impl Into<String>,
    ) -> Result<Self, StorageError> {
        let database_path = absolute_path(database_path.as_ref())?;
        let operation = operation.into();
        let path = database_operation_lock_path(&database_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut file = options.open(&path)?;
        validate_lock_file(&file, &path)?;
        if let Err(error) = file.try_lock_exclusive() {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                let mut active_operation = String::new();
                file.seek(SeekFrom::Start(0))?;
                file.read_to_string(&mut active_operation)?;
                let active_operation = active_operation.trim();
                return Err(StorageError::DatabaseOperationLocked {
                    operation,
                    active_operation: if active_operation.is_empty() {
                        "unknown operation".to_owned()
                    } else {
                        active_operation.to_owned()
                    },
                    database: database_path,
                });
            }
            return Err(error.into());
        }
        validate_lock_file(&file, &path)?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(operation.as_bytes())?;
        file.flush()?;
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn validate_lock_file(file: &File, path: &Path) -> Result<(), StorageError> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(StorageError::InvalidInput(format!(
            "database operation lock must be a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != 1 {
            return Err(StorageError::InvalidInput(format!(
                "database operation lock must not be hard-linked: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

impl Drop for DatabaseOperationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn database_operation_lock_path(path: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
    let database = absolute_path(path.as_ref())?;
    let name = database.file_name().ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "database path has no file name: {}",
            database.display()
        ))
    })?;
    Ok(database.with_file_name(format!(".{}.operation.lock", name.to_string_lossy())))
}

pub fn connect_pipeline_writer(path: impl AsRef<Path>) -> Result<Connection, StorageError> {
    connect_pipeline_writer_with_timeout(path, BUSY_TIMEOUT_MS)
}

pub fn connect_pipeline_writer_with_timeout(
    path: impl AsRef<Path>,
    busy_timeout_ms: u64,
) -> Result<Connection, StorageError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(busy_timeout_ms))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    Ok(connection)
}

pub fn connect_local_writer(path: impl AsRef<Path>) -> Result<Connection, StorageError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(connection)
}

pub fn connect_readonly(path: impl AsRef<Path>) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(
        absolute_path(path.as_ref())?,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(connection)
}

/// Run an immediate transaction and commit only when the operation succeeds.
pub fn in_transaction<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = operation(&transaction)?;
    transaction.commit()?;
    Ok(result)
}

/// Initialize every table owned by the pipeline persistence layer.
pub fn init_schema(connection: &Connection) -> Result<(), StorageError> {
    init_processed_inputs_table(connection)?;
    init_stats_tables(connection)?;
    init_datasets_table(connection)?;
    init_pipeline_product_table(connection)?;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, StorageError> {
    let expanded = if path.starts_with("~") {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            StorageError::InvalidInput(format!(
                "cannot expand path without a home: {}",
                path.display()
            ))
        })?;
        PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked"))
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in expanded.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            StorageError::InvalidInput(format!("cannot resolve path {}", path.display()))
        })?;
        suffix.push(name.to_owned());
        existing = existing.parent().ok_or_else(|| {
            StorageError::InvalidInput(format!("cannot resolve path {}", path.display()))
        })?;
    }
    let mut resolved = existing.canonicalize()?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Nfcapd,
    Csv,
}

impl InputKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nfcapd => "nfcapd",
            Self::Csv => "csv",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputStatus {
    Pending,
    Processed,
    Failed,
}

impl InputStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processed => "processed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputBucket {
    pub input_kind: InputKind,
    pub input_locator: String,
    pub scan_locator: String,
    pub source_id: String,
    pub bucket_start: i64,
    pub bucket_end: i64,
    pub revision: InputRevision,
    pub file_snapshot: Option<FileSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputBucketRef {
    pub source_id: String,
    pub bucket_start: i64,
}

pub fn init_processed_inputs_table(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS processed_inputs (
            input_kind TEXT NOT NULL CHECK (input_kind IN ('nfcapd', 'csv')),
            input_locator TEXT NOT NULL,
            scan_locator TEXT NOT NULL,
            source_id TEXT NOT NULL,
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processed', 'failed')),
            error_message TEXT,
            discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            processed_at DATETIME,
            content_fingerprint TEXT NOT NULL,
            decoder_fingerprint TEXT NOT NULL,
            revision_fingerprint TEXT NOT NULL,
            file_device INTEGER,
            file_inode INTEGER,
            file_size INTEGER,
            file_mtime_ns INTEGER,
            file_ctime_ns INTEGER,
            PRIMARY KEY (input_kind, input_locator, source_id, bucket_start)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS processed_input_scans (
            input_kind TEXT NOT NULL CHECK (input_kind IN ('csv')),
            input_locator TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('processed')),
            rejected_rows INTEGER NOT NULL DEFAULT 0,
            skipped_bad_column_count INTEGER NOT NULL DEFAULT 0,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            content_fingerprint TEXT NOT NULL,
            decoder_fingerprint TEXT NOT NULL,
            revision_fingerprint TEXT NOT NULL,
            file_device INTEGER,
            file_inode INTEGER,
            file_size INTEGER,
            file_mtime_ns INTEGER,
            file_ctime_ns INTEGER,
            PRIMARY KEY (input_kind, input_locator)
        ) WITHOUT ROWID;
        ",
    )?;
    ensure_column(
        connection,
        "processed_inputs",
        "status",
        "TEXT NOT NULL DEFAULT 'pending'",
    )?;
    ensure_column(connection, "processed_inputs", "error_message", "TEXT")?;
    ensure_column(connection, "processed_inputs", "processed_at", "DATETIME")?;
    ensure_column(connection, "processed_inputs", "scan_locator", "TEXT")?;
    ensure_column(
        connection,
        "processed_inputs",
        "content_fingerprint",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "processed_inputs",
        "decoder_fingerprint",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "processed_inputs",
        "revision_fingerprint",
        "TEXT",
    )?;
    for column in [
        "content_fingerprint",
        "decoder_fingerprint",
        "revision_fingerprint",
    ] {
        ensure_column(connection, "processed_input_scans", column, "TEXT")?;
    }
    for table in ["processed_inputs", "processed_input_scans"] {
        ensure_column(connection, table, "file_device", "INTEGER")?;
        ensure_column(connection, table, "file_inode", "INTEGER")?;
        ensure_column(connection, table, "file_size", "INTEGER")?;
        ensure_column(connection, table, "file_mtime_ns", "INTEGER")?;
        ensure_column(connection, table, "file_ctime_ns", "INTEGER")?;
    }
    ensure_column(
        connection,
        "processed_input_scans",
        "skipped_bad_column_count",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    connection.execute(
        "UPDATE processed_inputs SET scan_locator = input_locator WHERE scan_locator IS NULL",
        [],
    )?;
    connection.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_processed_inputs_source_bucket
        ON processed_inputs(source_id, bucket_start);
        CREATE INDEX IF NOT EXISTS idx_processed_inputs_scan_status
        ON processed_inputs(input_kind, scan_locator, status);
        ",
    )?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    column_type: &str,
) -> Result<(), StorageError> {
    validate_identifier(table)?;
    validate_identifier(column)?;
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    if !columns.contains(column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {column_type}"
        ))?;
    }
    Ok(())
}

fn validate_identifier(identifier: &str) -> Result<(), StorageError> {
    if identifier.is_empty()
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(StorageError::InvalidInput(format!(
            "invalid SQLite identifier: {identifier:?}"
        )));
    }
    Ok(())
}

pub fn upsert_input_bucket(
    connection: &Connection,
    bucket: &InputBucket,
    replace_revision: bool,
) -> Result<(), StorageError> {
    validate_revision_identity(&bucket.revision, bucket.input_kind, &bucket.input_locator)?;
    let existing = connection
        .query_row(
            "
            SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
            FROM processed_inputs
            WHERE input_kind = ?1 AND input_locator = ?2 AND source_id = ?3 AND bucket_start = ?4
            ",
            params![
                bucket.input_kind.as_str(),
                bucket.input_locator,
                bucket.source_id,
                bucket.bucket_start
            ],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some(stored) = existing
        && stored.2.as_deref() != Some(&bucket.revision.fingerprint)
        && !replace_revision
    {
        return Err(revision_conflict(
            &bucket.input_locator,
            &stored,
            &bucket.revision,
        ));
    }
    let snapshot = snapshot_values(bucket.file_snapshot.as_ref())?;
    connection.execute(
        "
        INSERT INTO processed_inputs (
            input_kind, input_locator, scan_locator, source_id, bucket_start, bucket_end, status,
            content_fingerprint, decoder_fingerprint, revision_fingerprint,
            file_device, file_inode, file_size, file_mtime_ns, file_ctime_ns
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT(input_kind, input_locator, source_id, bucket_start)
        DO UPDATE SET
            scan_locator = excluded.scan_locator,
            bucket_end = excluded.bucket_end,
            status = 'pending',
            error_message = NULL,
            processed_at = NULL,
            content_fingerprint = excluded.content_fingerprint,
            decoder_fingerprint = excluded.decoder_fingerprint,
            revision_fingerprint = excluded.revision_fingerprint,
            file_device = excluded.file_device,
            file_inode = excluded.file_inode,
            file_size = excluded.file_size,
            file_mtime_ns = excluded.file_mtime_ns,
            file_ctime_ns = excluded.file_ctime_ns
        ",
        params![
            bucket.input_kind.as_str(),
            bucket.input_locator,
            bucket.scan_locator,
            bucket.source_id,
            bucket.bucket_start,
            bucket.bucket_end,
            bucket.revision.content_fingerprint,
            bucket.revision.decoder_fingerprint,
            bucket.revision.fingerprint,
            snapshot[0],
            snapshot[1],
            snapshot[2],
            snapshot[3],
            snapshot[4],
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mark_input_bucket_status(
    connection: &Connection,
    input_kind: InputKind,
    input_locator: &str,
    source_id: &str,
    bucket_start: i64,
    status: InputStatus,
    revision: &InputRevision,
    error_message: Option<&str>,
) -> Result<(), StorageError> {
    validate_revision_identity(revision, input_kind, input_locator)?;
    let changed = connection.execute(
        "
        UPDATE processed_inputs
        SET status = ?1, error_message = ?2, processed_at = CURRENT_TIMESTAMP
        WHERE input_kind = ?3 AND input_locator = ?4 AND source_id = ?5 AND bucket_start = ?6
          AND revision_fingerprint = ?7
        ",
        params![
            status.as_str(),
            error_message,
            input_kind.as_str(),
            input_locator,
            source_id,
            bucket_start,
            revision.fingerprint,
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::InputRevisionConflict {
            locator: input_locator.to_owned(),
            components: "revision before status update".to_owned(),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn complete_input_scan(
    connection: &Connection,
    input_kind: InputKind,
    scan_locator: &str,
    rejected_rows: i64,
    skipped_bad_column_count: i64,
    revision: &InputRevision,
    file_snapshot: Option<&FileSnapshot>,
) -> Result<(), StorageError> {
    if input_kind != InputKind::Csv {
        return Err(StorageError::InvalidInput(format!(
            "Unsupported scanned input kind: {}",
            input_kind.as_str()
        )));
    }
    if rejected_rows < 0 {
        return Err(StorageError::InvalidInput(
            "rejected_rows must be non-negative".to_owned(),
        ));
    }
    if skipped_bad_column_count < 0 {
        return Err(StorageError::InvalidInput(
            "skipped_bad_column_count must be non-negative".to_owned(),
        ));
    }
    init_processed_inputs_table(connection)?;
    validate_revision_identity(revision, input_kind, scan_locator)?;
    let unfinished = connection.query_row(
        "
        SELECT COUNT(*) FROM processed_inputs
        WHERE input_kind = ?1 AND scan_locator = ?2
          AND (status != 'processed' OR content_fingerprint != ?3 OR decoder_fingerprint != ?4)
        ",
        params![
            input_kind.as_str(),
            scan_locator,
            revision.content_fingerprint,
            revision.decoder_fingerprint,
        ],
        |row| row.get::<_, i64>(0),
    )?;
    if unfinished != 0 {
        return Err(StorageError::InvalidInput(format!(
            "Cannot complete input scan {scan_locator:?}: {unfinished} bucket(s) are unfinished."
        )));
    }
    let existing = connection
        .query_row(
            "
            SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
            FROM processed_input_scans WHERE input_kind = ?1 AND input_locator = ?2
            ",
            params![input_kind.as_str(), scan_locator],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some(stored) = existing
        && stored.2.as_deref() != Some(&revision.fingerprint)
    {
        return Err(revision_conflict(scan_locator, &stored, revision));
    }
    let snapshot = snapshot_values(file_snapshot)?;
    connection.execute(
        "
        INSERT INTO processed_input_scans (
            input_kind, input_locator, status, rejected_rows, skipped_bad_column_count,
            processed_at, content_fingerprint, decoder_fingerprint, revision_fingerprint,
            file_device, file_inode, file_size, file_mtime_ns, file_ctime_ns
        ) VALUES (?1, ?2, 'processed', ?3, ?4, CURRENT_TIMESTAMP, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(input_kind, input_locator) DO UPDATE SET
            status = 'processed', rejected_rows = excluded.rejected_rows,
            skipped_bad_column_count = excluded.skipped_bad_column_count,
            processed_at = CURRENT_TIMESTAMP,
            content_fingerprint = excluded.content_fingerprint,
            decoder_fingerprint = excluded.decoder_fingerprint,
            revision_fingerprint = excluded.revision_fingerprint,
            file_device = excluded.file_device, file_inode = excluded.file_inode,
            file_size = excluded.file_size, file_mtime_ns = excluded.file_mtime_ns,
            file_ctime_ns = excluded.file_ctime_ns
        ",
        params![
            input_kind.as_str(),
            scan_locator,
            rejected_rows,
            skipped_bad_column_count,
            revision.content_fingerprint,
            revision.decoder_fingerprint,
            revision.fingerprint,
            snapshot[0],
            snapshot[1],
            snapshot[2],
            snapshot[3],
            snapshot[4],
        ],
    )?;
    Ok(())
}

pub fn cached_content_fingerprint(
    connection: &Connection,
    input_kind: InputKind,
    input_locator: &str,
    file_snapshot: &FileSnapshot,
) -> Result<Option<String>, StorageError> {
    let table = if input_kind == InputKind::Csv {
        "processed_input_scans"
    } else {
        "processed_inputs"
    };
    let snapshot = snapshot_values(Some(file_snapshot))?;
    Ok(connection
        .query_row(
            &format!(
                "
                SELECT content_fingerprint FROM {table}
                WHERE input_kind = ?1 AND input_locator = ?2 AND status = 'processed'
                  AND file_device = ?3 AND file_inode = ?4 AND file_size = ?5
                  AND file_mtime_ns = ?6 AND file_ctime_ns = ?7
                LIMIT 1
                "
            ),
            params![
                input_kind.as_str(),
                input_locator,
                snapshot[0],
                snapshot[1],
                snapshot[2],
                snapshot[3],
                snapshot[4]
            ],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn input_scan_fully_processed(
    connection: &Connection,
    input_kind: InputKind,
    scan_locator: &str,
    revision: &InputRevision,
) -> Result<bool, StorageError> {
    validate_revision_identity(revision, input_kind, scan_locator)?;
    let stored = connection
        .query_row(
            "
            SELECT content_fingerprint, decoder_fingerprint, revision_fingerprint
            FROM processed_input_scans WHERE input_kind = ?1 AND input_locator = ?2
            ",
            params![input_kind.as_str(), scan_locator],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some(stored) = stored else {
        return Ok(false);
    };
    if stored.2.as_deref() != Some(&revision.fingerprint) {
        return Err(revision_conflict(scan_locator, &stored, revision));
    }
    Ok(connection
        .query_row(
            "
            SELECT 1 FROM processed_input_scans AS scans
            WHERE scans.input_kind = ?1 AND scans.input_locator = ?2
              AND scans.revision_fingerprint = ?3 AND scans.status = 'processed'
              AND NOT EXISTS (
                  SELECT 1 FROM processed_inputs AS buckets
                  WHERE buckets.input_kind = scans.input_kind
                    AND buckets.scan_locator = scans.input_locator
                    AND (buckets.content_fingerprint != scans.content_fingerprint
                         OR buckets.decoder_fingerprint != scans.decoder_fingerprint
                         OR buckets.status != 'processed')
              )
            ",
            params![input_kind.as_str(), scan_locator, revision.fingerprint],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Check whether a logical nfcapd bucket was produced by exactly these revisions.
pub fn nfcapd_logical_bucket_processed(
    connection: &Connection,
    source_id: &str,
    bucket_start: i64,
    revisions: &[InputRevision],
) -> Result<bool, StorageError> {
    if revisions.is_empty() {
        return Ok(false);
    }
    let mut statement = connection.prepare(
        "
        SELECT input_locator, revision_fingerprint FROM processed_inputs
        WHERE input_kind = 'nfcapd' AND source_id = ?1 AND bucket_start = ?2
          AND status = 'processed'
        ",
    )?;
    let stored = statement
        .query_map(params![source_id, bucket_start], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let requested = revisions
        .iter()
        .map(|revision| (revision.locator.clone(), revision.fingerprint.clone()))
        .collect::<BTreeSet<_>>();
    let stored_locators = stored
        .iter()
        .map(|(locator, _)| locator)
        .collect::<BTreeSet<_>>();
    let requested_locators = requested
        .iter()
        .map(|(locator, _)| locator)
        .collect::<BTreeSet<_>>();
    if stored_locators == requested_locators && stored != requested {
        return Err(StorageError::InputRevisionConflict {
            locator: format!("{source_id}:{bucket_start}"),
            components: "nfcapd content or decoder; rerun with force to rewrite it".to_owned(),
        });
    }
    Ok(stored == requested)
}

pub fn clear_incomplete_input_scan(
    connection: &Connection,
    input_kind: InputKind,
    scan_locator: &str,
) -> Result<Vec<InputBucketRef>, StorageError> {
    init_processed_inputs_table(connection)?;
    let terminal = connection
        .query_row(
            "SELECT 1 FROM processed_input_scans WHERE input_kind = ?1 AND input_locator = ?2 AND status = 'processed'",
            params![input_kind.as_str(), scan_locator],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if terminal {
        return Err(StorageError::InvalidInput(format!(
            "Cannot clear successfully completed input scan {scan_locator:?}."
        )));
    }
    let mut statement = connection.prepare(
        "
        SELECT DISTINCT source_id, bucket_start FROM processed_inputs
        WHERE input_kind = ?1 AND scan_locator = ?2 ORDER BY source_id, bucket_start
        ",
    )?;
    let buckets = statement
        .query_map(params![input_kind.as_str(), scan_locator], |row| {
            Ok(InputBucketRef {
                source_id: row.get(0)?,
                bucket_start: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    connection.execute(
        "DELETE FROM processed_inputs WHERE input_kind = ?1 AND scan_locator = ?2",
        params![input_kind.as_str(), scan_locator],
    )?;
    Ok(buckets)
}

fn validate_revision_identity(
    revision: &InputRevision,
    input_kind: InputKind,
    locator: &str,
) -> Result<(), StorageError> {
    if revision.input_kind != input_kind.as_str() || revision.locator != locator {
        return Err(StorageError::InvalidInput(format!(
            "Input revision identity does not match persistence owner: {}:{} != {}:{}",
            revision.input_kind,
            revision.locator,
            input_kind.as_str(),
            locator,
        )));
    }
    Ok(())
}

fn revision_conflict(
    locator: &str,
    stored: &(Option<String>, Option<String>, Option<String>),
    requested: &InputRevision,
) -> StorageError {
    let mut mismatches = Vec::new();
    if stored.0.as_deref() != Some(&requested.content_fingerprint) {
        mismatches.push("content");
    }
    if stored.1.as_deref() != Some(&requested.decoder_fingerprint) {
        mismatches.push("decoder");
    }
    if mismatches.is_empty() {
        mismatches.push("combined revision");
    }
    StorageError::InputRevisionConflict {
        locator: locator.to_owned(),
        components: mismatches.join(", "),
    }
}

fn snapshot_values(snapshot: Option<&FileSnapshot>) -> Result<[Option<i64>; 5], StorageError> {
    let Some(snapshot) = snapshot else {
        return Ok([None; 5]);
    };
    Ok([
        Some(sqlite_uint64(snapshot.device)),
        Some(sqlite_uint64(snapshot.inode)),
        Some(i64::try_from(snapshot.size).map_err(|_| {
            StorageError::InvalidInput(format!(
                "file size is outside SQLite INTEGER: {}",
                snapshot.size
            ))
        })?),
        Some(snapshot.mtime_ns),
        Some(snapshot.ctime_ns),
    ])
}

/// Losslessly map an unsigned file stat identifier into SQLite's signed integer.
pub const fn sqlite_uint64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductIdentity {
    pub schema_json: String,
    pub schema_fingerprint: String,
    pub selection_json: String,
    pub selection_fingerprint: String,
    pub config_json: String,
    pub config_fingerprint: String,
    pub fingerprint: String,
}

impl ProductIdentity {
    pub fn create<S: Serialize + ?Sized, L: Serialize + ?Sized, C: Serialize + ?Sized>(
        schema: &S,
        selection: &L,
        config: &C,
    ) -> Result<Self, StorageError> {
        let schema_json = canonical_json(schema)?;
        let selection_json = canonical_json(selection)?;
        let config_json = canonical_json(config)?;
        let schema_fingerprint = fingerprint(schema)?;
        let selection_fingerprint = fingerprint(selection)?;
        let config_fingerprint = fingerprint(config)?;
        let product_fingerprint = fingerprint(&serde_json::json!({
            "version": 1,
            "schema": schema_fingerprint,
            "selection": selection_fingerprint,
            "config": config_fingerprint,
        }))?;
        Ok(Self {
            schema_json,
            schema_fingerprint,
            selection_json,
            selection_fingerprint,
            config_json,
            config_fingerprint,
            fingerprint: product_fingerprint,
        })
    }
}

pub fn init_pipeline_product_table(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS pipeline_product (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            schema_json TEXT NOT NULL,
            schema_fingerprint TEXT NOT NULL,
            selection_json TEXT NOT NULL,
            selection_fingerprint TEXT NOT NULL,
            config_json TEXT NOT NULL,
            config_fingerprint TEXT NOT NULL,
            product_fingerprint TEXT NOT NULL,
            bound_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        ",
    )?;
    Ok(())
}

pub fn bind_product_identity(
    connection: &Connection,
    identity: &ProductIdentity,
    output_table_names: &[&str],
) -> Result<(), StorageError> {
    init_pipeline_product_table(connection)?;
    let stored = connection
        .query_row(
            "
            SELECT schema_json, schema_fingerprint, selection_json, selection_fingerprint,
                   config_json, config_fingerprint, product_fingerprint
            FROM pipeline_product WHERE singleton = 1
            ",
            [],
            |row| {
                Ok(ProductIdentity {
                    schema_json: row.get(0)?,
                    schema_fingerprint: row.get(1)?,
                    selection_json: row.get(2)?,
                    selection_fingerprint: row.get(3)?,
                    config_json: row.get(4)?,
                    config_fingerprint: row.get(5)?,
                    fingerprint: row.get(6)?,
                })
            },
        )
        .optional()?;
    let Some(stored) = stored else {
        let mut populated = Vec::new();
        for table in output_table_names
            .iter()
            .copied()
            .chain(["processed_inputs", "processed_input_scans"])
        {
            if table_has_rows(connection, table)? {
                populated.push(table);
            }
        }
        if !populated.is_empty() {
            return Err(StorageError::ProductIdentityConflict {
                components: "populated legacy database".to_owned(),
                details: format!(
                    "Cannot bind product identity; existing rows found in: {}. Use a new database.",
                    populated.join(", ")
                ),
            });
        }
        connection.execute(
            "
            INSERT INTO pipeline_product (
                singleton, schema_json, schema_fingerprint, selection_json,
                selection_fingerprint, config_json, config_fingerprint, product_fingerprint
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ",
            params![
                identity.schema_json,
                identity.schema_fingerprint,
                identity.selection_json,
                identity.selection_fingerprint,
                identity.config_json,
                identity.config_fingerprint,
                identity.fingerprint,
            ],
        )?;
        return Ok(());
    };
    if stored.fingerprint == identity.fingerprint {
        return Ok(());
    }
    let mut mismatches = Vec::new();
    let mut details = Vec::new();
    for (name, stored_fingerprint, requested_fingerprint, stored_json, requested_json) in [
        (
            "schema",
            &stored.schema_fingerprint,
            &identity.schema_fingerprint,
            &stored.schema_json,
            &identity.schema_json,
        ),
        (
            "selection",
            &stored.selection_fingerprint,
            &identity.selection_fingerprint,
            &stored.selection_json,
            &identity.selection_json,
        ),
        (
            "config",
            &stored.config_fingerprint,
            &identity.config_fingerprint,
            &stored.config_json,
            &identity.config_json,
        ),
    ] {
        if stored_fingerprint != requested_fingerprint {
            mismatches.push(name);
            details.push(format!(
                "{name}: stored={stored_json} requested={requested_json}"
            ));
        }
    }
    Err(StorageError::ProductIdentityConflict {
        components: mismatches.join(", "),
        details: details.join("; "),
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceDefinition {
    pub source_id: String,
    pub members: Vec<String>,
}

impl SourceDefinition {
    pub fn new<S, M, I>(source_id: S, members: I) -> Self
    where
        S: Into<String>,
        M: Into<String>,
        I: IntoIterator<Item = M>,
    {
        Self {
            source_id: source_id.into(),
            members: members.into_iter().map(Into::into).collect(),
        }
    }
}

pub fn bind_nfcapd_source_layout(
    connection: &Connection,
    sources: &[SourceDefinition],
) -> Result<(), StorageError> {
    let mut normalized = sources.to_vec();
    normalized.sort_unstable_by(|left, right| left.source_id.cmp(&right.source_id));
    for source in &mut normalized {
        source.members.sort_unstable();
    }
    let layout = serde_json::json!({"version": 1, "sources": normalized});
    let layout_json = canonical_json(&layout)?;
    let layout_fingerprint = fingerprint(&layout)?;
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS nfcapd_source_layout (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            layout_json TEXT NOT NULL,
            layout_fingerprint TEXT NOT NULL,
            bound_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        ",
    )?;
    let stored = connection
        .query_row(
            "SELECT layout_json, layout_fingerprint FROM nfcapd_source_layout WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((stored_json, stored_fingerprint)) = stored else {
        if table_has_rows_where(connection, "processed_inputs", "input_kind = 'nfcapd'")? {
            return Err(StorageError::SourceLayoutConflict(
                "Cannot bind nfcapd source layout after unbound nfcapd inputs were processed. Use a new database."
                    .to_owned(),
            ));
        }
        connection.execute(
            "INSERT INTO nfcapd_source_layout (singleton, layout_json, layout_fingerprint) VALUES (1, ?1, ?2)",
            params![layout_json, layout_fingerprint],
        )?;
        return Ok(());
    };
    if stored_fingerprint != layout_fingerprint {
        return Err(StorageError::SourceLayoutConflict(format!(
            "nfcapd logical source membership changed. stored={stored_json} requested={layout_json}. Use a new database."
        )));
    }
    Ok(())
}

fn table_has_rows(connection: &Connection, table: &str) -> Result<bool, StorageError> {
    table_has_rows_where(connection, table, "1")
}

fn table_has_rows_where(
    connection: &Connection,
    table: &str,
    predicate: &str,
) -> Result<bool, StorageError> {
    validate_identifier(table)?;
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(false);
    }
    Ok(connection
        .query_row(
            &format!("SELECT 1 FROM {table} WHERE {predicate} LIMIT 1"),
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn init_stats_tables(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS traffic_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            flows INTEGER NOT NULL,
            flows_tcp INTEGER NOT NULL,
            flows_udp INTEGER NOT NULL,
            flows_icmp INTEGER NOT NULL,
            flows_other INTEGER NOT NULL,
            packets INTEGER NOT NULL,
            packets_tcp INTEGER NOT NULL,
            packets_udp INTEGER NOT NULL,
            packets_icmp INTEGER NOT NULL,
            packets_other INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            bytes_tcp INTEGER NOT NULL,
            bytes_udp INTEGER NOT NULL,
            bytes_icmp INTEGER NOT NULL,
            bytes_other INTEGER NOT NULL,
            duration_sum_ms INTEGER NOT NULL,
            duration_count INTEGER NOT NULL,
            average_duration_ms REAL,
            min_ttl_sum INTEGER NOT NULL,
            min_ttl_count INTEGER NOT NULL,
            average_min_ttl REAL,
            max_ttl_sum INTEGER NOT NULL,
            max_ttl_count INTEGER NOT NULL,
            average_max_ttl REAL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_traffic_stats_query
        ON traffic_stats (granularity, bucket_start, source_id, ip_version, src_visibility, dst_visibility);

        CREATE TABLE IF NOT EXISTS protocol_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            unique_protocols_count INTEGER NOT NULL,
            protocols_list TEXT NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_protocol_stats_query
        ON protocol_stats (granularity, bucket_start, source_id, ip_version, src_visibility, dst_visibility);

        CREATE TABLE IF NOT EXISTS address_count_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            address_side TEXT NOT NULL CHECK (address_side IN ('source', 'destination')),
            unique_address_count INTEGER NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility, address_side)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_address_count_stats_query
        ON address_count_stats (granularity, bucket_start, source_id, ip_version, src_visibility, dst_visibility, address_side);

        CREATE TABLE IF NOT EXISTS port_count_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            port_side TEXT NOT NULL CHECK (port_side IN ('source', 'destination')),
            port_range TEXT NOT NULL CHECK (port_range IN ('low', 'high')),
            unique_port_count INTEGER NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility, port_side, port_range)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_port_count_stats_query
        ON port_count_stats (granularity, bucket_start, source_id, ip_version, src_visibility, dst_visibility, port_side, port_range);

        CREATE TABLE IF NOT EXISTS address_structure_stats (
            source_id TEXT NOT NULL,
            granularity TEXT NOT NULL CHECK (granularity IN ('5m', '30m', '1h', '1d')),
            bucket_start INTEGER NOT NULL,
            bucket_end INTEGER NOT NULL,
            ip_version INTEGER NOT NULL CHECK (ip_version IN (4, 6)),
            src_visibility TEXT NOT NULL CHECK (src_visibility IN ('all', 'literal', 'anonymized')),
            dst_visibility TEXT NOT NULL CHECK (dst_visibility IN ('all', 'literal', 'anonymized')),
            address_side TEXT NOT NULL CHECK (address_side IN ('source', 'destination')),
            structure_kind TEXT NOT NULL CHECK (structure_kind IN ('structure', 'spectrum', 'dimension')),
            values_json TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            processed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_id, granularity, bucket_start, ip_version, src_visibility, dst_visibility, address_side, structure_kind)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_address_structure_stats_query
        ON address_structure_stats (granularity, bucket_start, source_id, ip_version, src_visibility, dst_visibility, address_side, structure_kind);
        ",
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsDimensions {
    pub source_id: String,
    pub granularity: String,
    pub bucket_start: i64,
    pub bucket_end: i64,
    pub ip_version: i64,
    pub src_visibility: String,
    pub dst_visibility: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrafficStatsRow {
    pub dimensions: StatsDimensions,
    pub flows: i64,
    pub flows_tcp: i64,
    pub flows_udp: i64,
    pub flows_icmp: i64,
    pub flows_other: i64,
    pub packets: i64,
    pub packets_tcp: i64,
    pub packets_udp: i64,
    pub packets_icmp: i64,
    pub packets_other: i64,
    pub bytes: i64,
    pub bytes_tcp: i64,
    pub bytes_udp: i64,
    pub bytes_icmp: i64,
    pub bytes_other: i64,
    pub duration_sum_ms: i64,
    pub duration_count: i64,
    pub average_duration_ms: Option<f64>,
    pub min_ttl_sum: i64,
    pub min_ttl_count: i64,
    pub average_min_ttl: Option<f64>,
    pub max_ttl_sum: i64,
    pub max_ttl_count: i64,
    pub average_max_ttl: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolStatsRow {
    pub dimensions: StatsDimensions,
    pub unique_protocols_count: i64,
    pub protocols_list: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressCountStatsRow {
    pub dimensions: StatsDimensions,
    pub address_side: String,
    pub unique_address_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortCountStatsRow {
    pub dimensions: StatsDimensions,
    pub port_side: String,
    pub port_range: String,
    pub unique_port_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddressStructureStatsRow {
    pub dimensions: StatsDimensions,
    pub address_side: String,
    pub structure_kind: String,
    pub values_json: String,
    pub metadata_json: String,
}

pub fn insert_traffic_stats_rows(
    connection: &Connection,
    rows: &[TrafficStatsRow],
) -> Result<(), StorageError> {
    let mut statement = connection.prepare_cached(
        "
        INSERT OR REPLACE INTO traffic_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility,
            flows, flows_tcp, flows_udp, flows_icmp, flows_other,
            packets, packets_tcp, packets_udp, packets_icmp, packets_other,
            bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other,
            duration_sum_ms, duration_count, average_duration_ms,
            min_ttl_sum, min_ttl_count, average_min_ttl,
            max_ttl_sum, max_ttl_count, average_max_ttl
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31)
        ",
    )?;
    for row in rows {
        let dimensions = &row.dimensions;
        statement.execute(params![
            dimensions.source_id,
            dimensions.granularity,
            dimensions.bucket_start,
            dimensions.bucket_end,
            dimensions.ip_version,
            dimensions.src_visibility,
            dimensions.dst_visibility,
            row.flows,
            row.flows_tcp,
            row.flows_udp,
            row.flows_icmp,
            row.flows_other,
            row.packets,
            row.packets_tcp,
            row.packets_udp,
            row.packets_icmp,
            row.packets_other,
            row.bytes,
            row.bytes_tcp,
            row.bytes_udp,
            row.bytes_icmp,
            row.bytes_other,
            row.duration_sum_ms,
            row.duration_count,
            row.average_duration_ms,
            row.min_ttl_sum,
            row.min_ttl_count,
            row.average_min_ttl,
            row.max_ttl_sum,
            row.max_ttl_count,
            row.average_max_ttl,
        ])?;
    }
    Ok(())
}

pub fn insert_protocol_stats_rows(
    connection: &Connection,
    rows: &[ProtocolStatsRow],
) -> Result<(), StorageError> {
    let mut statement = connection.prepare_cached(
        "
        INSERT OR REPLACE INTO protocol_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, unique_protocols_count, protocols_list
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ",
    )?;
    for row in rows {
        let d = &row.dimensions;
        statement.execute(params![
            d.source_id,
            d.granularity,
            d.bucket_start,
            d.bucket_end,
            d.ip_version,
            d.src_visibility,
            d.dst_visibility,
            row.unique_protocols_count,
            row.protocols_list
        ])?;
    }
    Ok(())
}

pub fn insert_address_count_stats_rows(
    connection: &Connection,
    rows: &[AddressCountStatsRow],
) -> Result<(), StorageError> {
    let mut statement = connection.prepare_cached(
        "
        INSERT OR REPLACE INTO address_count_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, address_side, unique_address_count
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ",
    )?;
    for row in rows {
        let d = &row.dimensions;
        statement.execute(params![
            d.source_id,
            d.granularity,
            d.bucket_start,
            d.bucket_end,
            d.ip_version,
            d.src_visibility,
            d.dst_visibility,
            row.address_side,
            row.unique_address_count
        ])?;
    }
    Ok(())
}

pub fn insert_port_count_stats_rows(
    connection: &Connection,
    rows: &[PortCountStatsRow],
) -> Result<(), StorageError> {
    let mut statement = connection.prepare_cached(
        "
        INSERT OR REPLACE INTO port_count_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, port_side, port_range, unique_port_count
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ",
    )?;
    for row in rows {
        let d = &row.dimensions;
        statement.execute(params![
            d.source_id,
            d.granularity,
            d.bucket_start,
            d.bucket_end,
            d.ip_version,
            d.src_visibility,
            d.dst_visibility,
            row.port_side,
            row.port_range,
            row.unique_port_count
        ])?;
    }
    Ok(())
}

pub fn insert_address_structure_stats_rows(
    connection: &Connection,
    rows: &[AddressStructureStatsRow],
) -> Result<(), StorageError> {
    let mut statement = connection.prepare_cached(
        "
        INSERT OR REPLACE INTO address_structure_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility, address_side, structure_kind,
            values_json, metadata_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ",
    )?;
    for row in rows {
        let d = &row.dimensions;
        statement.execute(params![
            d.source_id,
            d.granularity,
            d.bucket_start,
            d.bucket_end,
            d.ip_version,
            d.src_visibility,
            d.dst_visibility,
            row.address_side,
            row.structure_kind,
            row.values_json,
            row.metadata_json
        ])?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsPayload {
    pub traffic_rows: Vec<TrafficStatsRow>,
    pub protocol_rows: Vec<ProtocolStatsRow>,
    pub address_count_rows: Vec<AddressCountStatsRow>,
    pub port_count_rows: Vec<PortCountStatsRow>,
    pub address_structure_rows: Vec<AddressStructureStatsRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsTable {
    Traffic,
    Protocol,
    AddressCount,
    PortCount,
    AddressStructure,
}

impl StatsTable {
    pub const ALL: [Self; 5] = [
        Self::Traffic,
        Self::Protocol,
        Self::AddressCount,
        Self::PortCount,
        Self::AddressStructure,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Traffic => "traffic_stats",
            Self::Protocol => "protocol_stats",
            Self::AddressCount => "address_count_stats",
            Self::PortCount => "port_count_stats",
            Self::AddressStructure => "address_structure_stats",
        }
    }

    pub const fn schema_version(self) -> u32 {
        match self {
            Self::Traffic => 2,
            Self::Protocol | Self::AddressCount | Self::PortCount | Self::AddressStructure => 1,
        }
    }
}

pub fn insert_stats_payload(
    connection: &Connection,
    payload: &StatsPayload,
    selected_tables: Option<&[StatsTable]>,
) -> Result<(), StorageError> {
    let selected = selected_tables.unwrap_or(&StatsTable::ALL);
    for table in selected {
        match table {
            StatsTable::Traffic => insert_traffic_stats_rows(connection, &payload.traffic_rows)?,
            StatsTable::Protocol => insert_protocol_stats_rows(connection, &payload.protocol_rows)?,
            StatsTable::AddressCount => {
                insert_address_count_stats_rows(connection, &payload.address_count_rows)?
            }
            StatsTable::PortCount => {
                insert_port_count_stats_rows(connection, &payload.port_count_rows)?
            }
            StatsTable::AddressStructure => {
                insert_address_structure_stats_rows(connection, &payload.address_structure_rows)?
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatsBucketKey {
    pub source_id: String,
    pub granularity: String,
    pub bucket_start: i64,
}

impl StatsBucketKey {
    pub fn new(
        source_id: impl Into<String>,
        granularity: impl Into<String>,
        bucket_start: i64,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            granularity: granularity.into(),
            bucket_start,
        }
    }
}

pub fn delete_stats_bucket_keys(
    connection: &Connection,
    keys: &[StatsBucketKey],
) -> Result<(), StorageError> {
    if keys.is_empty() {
        return Ok(());
    }
    for table in STATS_TABLE_NAMES {
        let mut statement = connection.prepare_cached(&format!(
            "DELETE FROM {table} WHERE source_id = ?1 AND granularity = ?2 AND bucket_start = ?3"
        ))?;
        for key in keys {
            statement.execute(params![key.source_id, key.granularity, key.bucket_start])?;
        }
    }
    Ok(())
}

#[cfg(test)]
impl StatsDimensions {
    fn example() -> Self {
        Self {
            source_id: "r1".into(),
            granularity: "5m".into(),
            bucket_start: 0,
            bucket_end: 300,
            ip_version: 4,
            src_visibility: "all".into(),
            dst_visibility: "all".into(),
        }
    }
}

#[cfg(test)]
impl TrafficStatsRow {
    fn example() -> Self {
        Self {
            dimensions: StatsDimensions::example(),
            flows: 2,
            flows_tcp: 2,
            flows_udp: 0,
            flows_icmp: 0,
            flows_other: 0,
            packets: 3,
            packets_tcp: 3,
            packets_udp: 0,
            packets_icmp: 0,
            packets_other: 0,
            bytes: 4,
            bytes_tcp: 4,
            bytes_udp: 0,
            bytes_icmp: 0,
            bytes_other: 0,
            duration_sum_ms: 10,
            duration_count: 2,
            average_duration_ms: Some(5.0),
            min_ttl_sum: 62,
            min_ttl_count: 2,
            average_min_ttl: Some(31.0),
            max_ttl_sum: 128,
            max_ttl_count: 2,
            average_max_ttl: Some(64.0),
        }
    }
}

#[cfg(test)]
impl ProtocolStatsRow {
    fn example() -> Self {
        Self {
            dimensions: StatsDimensions::example(),
            unique_protocols_count: 1,
            protocols_list: "6".into(),
        }
    }
}

#[cfg(test)]
impl AddressCountStatsRow {
    fn example() -> Self {
        Self {
            dimensions: StatsDimensions::example(),
            address_side: "source".into(),
            unique_address_count: 1,
        }
    }
}

#[cfg(test)]
impl PortCountStatsRow {
    fn example() -> Self {
        Self {
            dimensions: StatsDimensions::example(),
            port_side: "source".into(),
            port_range: "low".into(),
            unique_port_count: 1,
        }
    }
}

#[cfg(test)]
impl AddressStructureStatsRow {
    fn example() -> Self {
        Self {
            dimensions: StatsDimensions::example(),
            address_side: "source".into(),
            structure_kind: "structure".into(),
            values_json: "[]".into(),
            metadata_json: "{}".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetMetadata {
    pub dataset_id: String,
    pub label: String,
    pub default_start_date: String,
    pub source_mode: String,
    pub discovery_mode: String,
    pub sort_order: i64,
    pub sources: Vec<SourceDefinition>,
}

impl DatasetMetadata {
    pub fn new(dataset_id: impl Into<String>) -> Self {
        let dataset_id = dataset_id.into();
        Self {
            label: dataset_id.clone(),
            dataset_id,
            default_start_date: "2025-02-01".to_owned(),
            source_mode: "static".to_owned(),
            discovery_mode: "static".to_owned(),
            sort_order: 0,
            sources: Vec::new(),
        }
    }

    pub fn with_sources<S>(mut self, sources: S) -> Self
    where
        S: IntoIterator<Item = SourceDefinition>,
    {
        self.sources = sources.into_iter().collect();
        self
    }
}

pub fn init_datasets_table(connection: &Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS datasets (
            id TEXT PRIMARY KEY NOT NULL,
            label TEXT NOT NULL,
            default_start_date TEXT NOT NULL,
            source_mode TEXT NOT NULL DEFAULT 'static',
            discovery_mode TEXT NOT NULL DEFAULT 'static',
            sort_order INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS source_members (
            dataset_id TEXT NOT NULL,
            source_id TEXT NOT NULL,
            member_id TEXT NOT NULL,
            PRIMARY KEY (dataset_id, source_id, member_id)
        );
        ",
    )?;
    Ok(())
}

pub fn upsert_dataset_metadata(
    connection: &Connection,
    dataset: &DatasetMetadata,
) -> Result<(), StorageError> {
    init_datasets_table(connection)?;
    let dataset_id = dataset.dataset_id.trim();
    let label = if dataset.label.is_empty() {
        dataset_id
    } else {
        &dataset.label
    };
    let default_start_date = if dataset.default_start_date.is_empty() {
        "2025-02-01"
    } else {
        dataset.default_start_date.trim()
    };
    let source_mode = if dataset.source_mode.is_empty() {
        "static"
    } else {
        dataset.source_mode.trim()
    };
    let discovery_mode = if dataset.discovery_mode.is_empty() {
        "static"
    } else {
        dataset.discovery_mode.trim()
    };
    connection.execute(
        "
        INSERT INTO datasets (id, label, default_start_date, source_mode, discovery_mode, sort_order)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            default_start_date = excluded.default_start_date,
            source_mode = excluded.source_mode,
            discovery_mode = excluded.discovery_mode,
            sort_order = excluded.sort_order
        ",
        params![dataset_id, label, default_start_date, source_mode, discovery_mode, dataset.sort_order],
    )?;
    upsert_source_members(connection, dataset_id, &dataset.sources)
}

/// Replace logical source membership metadata for one dataset.
pub fn upsert_source_members(
    connection: &Connection,
    dataset_id: &str,
    sources: &[SourceDefinition],
) -> Result<(), StorageError> {
    connection.execute(
        "DELETE FROM source_members WHERE dataset_id = ?1",
        [dataset_id],
    )?;
    let mut statement = connection.prepare_cached(
        "INSERT OR REPLACE INTO source_members (dataset_id, source_id, member_id) VALUES (?1, ?2, ?3)",
    )?;
    for source in sources {
        let source_id = source.source_id.trim();
        if source_id.is_empty() {
            continue;
        }
        for member in &source.members {
            let member = member.trim();
            if !member.is_empty() {
                statement.execute(params![dataset_id, source_id, member])?;
            }
        }
    }
    Ok(())
}

/// Create a consistent SQLite backup and atomically replace the target.
pub fn backup_database(
    source_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<(), StorageError> {
    let (source_path, target_path) = resolved_backup_paths(source_path, target_path)?;
    let _locks = acquire_database_operation_locks(
        [source_path.as_path(), target_path.as_path()],
        format!(
            "database backup {} -> {}",
            source_path.display(),
            target_path.display()
        ),
    )?;
    publish_backup(&source_path, &target_path)
}

/// Back up an existing target, then publish a consistent candidate snapshot.
pub fn promote_database(
    candidate_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
    backup_existing_path: Option<&Path>,
) -> Result<(), StorageError> {
    let (candidate_path, target_path) = resolved_backup_paths(candidate_path, target_path)?;
    let backup_existing_path = backup_existing_path.map(absolute_path).transpose()?;
    let mut database_paths = vec![candidate_path.as_path(), target_path.as_path()];
    database_paths.extend(backup_existing_path.as_deref());
    validate_database_path_separation(&database_paths)?;
    let mut lock_paths = vec![candidate_path.as_path(), target_path.as_path()];
    if let Some(path) = backup_existing_path.as_deref() {
        lock_paths.push(path);
    }
    let _locks = acquire_database_operation_locks(
        lock_paths,
        format!("database promotion to {}", target_path.display()),
    )?;
    if target_path.exists()
        && let Some(backup_existing_path) = backup_existing_path.as_deref()
    {
        publish_backup(&target_path, backup_existing_path)?;
    }
    publish_backup(&candidate_path, &target_path)
}

fn resolved_backup_paths(
    source_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<(PathBuf, PathBuf), StorageError> {
    let source_path = absolute_path(source_path.as_ref())?;
    let target_path = absolute_path(target_path.as_ref())?;
    validate_database_path_separation(&[source_path.as_path(), target_path.as_path()])?;
    if !source_path.is_file() {
        return Err(StorageError::DatabaseNotFound(source_path));
    }
    Ok((source_path, target_path))
}

fn validate_database_path_separation(paths: &[&Path]) -> Result<(), StorageError> {
    let mut claimed = BTreeMap::new();
    for path in paths {
        for related in database_related_paths(path)? {
            if let Some(owner) = claimed.insert(related.clone(), (*path).to_owned()) {
                return Err(StorageError::InvalidInput(format!(
                    "database paths and their SQLite sidecar/operation-lock paths must be distinct: {} aliases {} through {}",
                    owner.display(),
                    path.display(),
                    related.display()
                )));
            }
        }
    }
    Ok(())
}

fn database_related_paths(path: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut paths = vec![path.to_owned(), database_operation_lock_path(path)?];
    paths.extend(["-journal", "-wal", "-shm"].map(|suffix| sidecar_path(path, suffix)));
    paths
        .into_iter()
        .map(|related| absolute_path(&related))
        .collect()
}

fn acquire_database_operation_locks<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    operation: String,
) -> Result<Vec<DatabaseOperationLock>, StorageError> {
    let paths = paths
        .into_iter()
        .map(absolute_path)
        .collect::<Result<BTreeSet<_>, _>>()?;
    paths
        .into_iter()
        .map(|path| DatabaseOperationLock::acquire(path, operation.clone()))
        .collect()
}

fn publish_backup(source_path: &Path, target_path: &Path) -> Result<(), StorageError> {
    let parent = target_path.parent().ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "target database path has no parent: {}",
            target_path.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let target_name = target_path.file_name().ok_or_else(|| {
        StorageError::InvalidInput(format!(
            "target database path has no file name: {}",
            target_path.display()
        ))
    })?;
    let temporary = Builder::new()
        .prefix(&format!(".{}.", target_name.to_string_lossy()))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    let (_, temporary_path) = temporary.keep().map_err(|error| error.error)?;
    let result = (|| {
        let source = connect_readonly(source_path)?;
        let mut target = connect_local_writer(&temporary_path)?;
        {
            let backup = Backup::new(&source, &mut target)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
        }
        let quick_check =
            target.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
        if quick_check != "ok" {
            return Err(StorageError::InvalidInput(format!(
                "Backup quick_check failed: {quick_check:?}"
            )));
        }
        drop(target);
        drop(source);
        atomic_replace_sqlite(&temporary_path, target_path)
    })();
    if result.is_err() {
        remove_if_exists(&temporary_path)?;
        for suffix in ["-journal", "-wal", "-shm"] {
            remove_if_exists(&sidecar_path(&temporary_path, suffix))?;
        }
    }
    result
}

pub fn atomic_replace_sqlite(
    source_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<(), StorageError> {
    let source_path = absolute_path(source_path.as_ref())?;
    let target_path = absolute_path(target_path.as_ref())?;
    validate_database_path_separation(&[source_path.as_path(), target_path.as_path()])?;
    let mut displaced = Vec::new();
    for suffix in ["-journal", "-wal", "-shm"] {
        let target_sidecar = sidecar_path(&target_path, suffix);
        if !target_sidecar.exists() {
            continue;
        }
        let displaced_sidecar = sidecar_path(&source_path, &format!(".previous{suffix}"));
        if let Err(error) = fs::rename(&target_sidecar, &displaced_sidecar) {
            restore_sidecars(&displaced);
            return Err(error.into());
        }
        displaced.push((target_sidecar, displaced_sidecar));
    }
    if let Err(error) = fs::rename(&source_path, &target_path) {
        restore_sidecars(&displaced);
        return Err(error.into());
    }
    for (_, path) in displaced {
        remove_if_exists(&path)?;
    }
    Ok(())
}

fn restore_sidecars(displaced: &[(PathBuf, PathBuf)]) {
    for (original, temporary) in displaced.iter().rev() {
        let _ = fs::rename(temporary, original);
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    path.with_file_name(format!(
        "{}{suffix}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn remove_if_exists(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn revision(locator: &str, content: &str) -> InputRevision {
        InputRevision::create("csv", locator, content, "decoder").unwrap()
    }

    #[test]
    fn writer_and_readonly_connections_preserve_wal_snapshot_isolation() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("netflow.sqlite");
        let writer = connect_pipeline_writer(&path).unwrap();
        writer
            .execute("CREATE TABLE events (value TEXT NOT NULL)", [])
            .unwrap();
        writer
            .execute("INSERT INTO events VALUES ('before')", [])
            .unwrap();
        let reader = connect_readonly(&path).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM events", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "before"
        );

        writer
            .execute("INSERT INTO events VALUES ('after')", [])
            .unwrap();
        let count = reader
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            reader
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))
                .unwrap(),
            BUSY_TIMEOUT_MS as i64
        );
        assert!(
            reader
                .execute("INSERT INTO events VALUES ('forbidden')", [])
                .is_err()
        );
    }

    #[test]
    fn operation_lock_is_nonblocking_and_reports_the_owner() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("netflow.sqlite");
        let first = DatabaseOperationLock::acquire(&path, "pipeline build").unwrap();
        let error = DatabaseOperationLock::acquire(&path, "backup").unwrap_err();
        assert!(error.to_string().contains("pipeline build"));
        drop(first);
        DatabaseOperationLock::acquire(&path, "backup").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn operation_lock_rejects_a_hard_link_to_the_database_before_truncating() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("netflow.sqlite");
        fs::write(&database, b"valuable database bytes").unwrap();
        let lock = database_operation_lock_path(&database).unwrap();
        fs::hard_link(&database, &lock).unwrap();
        let original = fs::read(&database).unwrap();

        let error = DatabaseOperationLock::acquire(&database, "pipeline build").unwrap_err();

        assert!(error.to_string().contains("must not be hard-linked"));
        assert_eq!(fs::read(database).unwrap(), original);
    }

    #[test]
    fn product_and_source_layout_bind_once_to_empty_database() {
        let connection = Connection::open_in_memory().unwrap();
        init_schema(&connection).unwrap();
        let identity = ProductIdentity::create(
            &json!({"version": 1, "tables": ["traffic_stats"]}),
            &json!({"version": 1, "kind": "all"}),
            &json!({"version": 1, "timezone": "UTC"}),
        )
        .unwrap();
        bind_product_identity(&connection, &identity, &STATS_TABLE_NAMES).unwrap();
        bind_product_identity(&connection, &identity, &STATS_TABLE_NAMES).unwrap();
        let changed = ProductIdentity::create(
            &json!({"version": 1, "tables": ["traffic_stats"]}),
            &json!({"version": 1, "kind": "none"}),
            &json!({"version": 1, "timezone": "UTC"}),
        )
        .unwrap();
        assert!(matches!(
            bind_product_identity(&connection, &changed, &STATS_TABLE_NAMES),
            Err(StorageError::ProductIdentityConflict { .. })
        ));

        let layout = vec![SourceDefinition::new("r1", ["member-b", "member-a"])];
        bind_nfcapd_source_layout(&connection, &layout).unwrap();
        assert!(
            bind_nfcapd_source_layout(&connection, &[SourceDefinition::new("r1", ["other"])])
                .is_err()
        );
    }

    #[test]
    fn scan_completion_requires_matching_processed_buckets() {
        let connection = Connection::open_in_memory().unwrap();
        init_processed_inputs_table(&connection).unwrap();
        let scan = revision("/csv/input.csv", "content");
        let bucket = revision_for_bucket(&scan, "archive://member.csv");
        upsert_input_bucket(
            &connection,
            &InputBucket {
                input_kind: InputKind::Csv,
                input_locator: bucket.locator.clone(),
                scan_locator: scan.locator.clone(),
                source_id: "r1".into(),
                bucket_start: 300,
                bucket_end: 600,
                revision: bucket.clone(),
                file_snapshot: None,
            },
            false,
        )
        .unwrap();
        assert!(
            complete_input_scan(
                &connection,
                InputKind::Csv,
                &scan.locator,
                0,
                0,
                &scan,
                None
            )
            .is_err()
        );
        mark_input_bucket_status(
            &connection,
            InputKind::Csv,
            &bucket.locator,
            "r1",
            300,
            InputStatus::Processed,
            &bucket,
            None,
        )
        .unwrap();
        complete_input_scan(
            &connection,
            InputKind::Csv,
            &scan.locator,
            2,
            1,
            &scan,
            None,
        )
        .unwrap();
        assert!(
            input_scan_fully_processed(&connection, InputKind::Csv, &scan.locator, &scan).unwrap()
        );
        assert!(matches!(
            input_scan_fully_processed(
                &connection,
                InputKind::Csv,
                &scan.locator,
                &revision("/csv/input.csv", "replacement")
            ),
            Err(StorageError::InputRevisionConflict { .. })
        ));
    }

    fn revision_for_bucket(scan: &InputRevision, locator: &str) -> InputRevision {
        crate::provenance::revision_for_locator(scan, locator).unwrap()
    }

    #[test]
    fn unsigned_file_identifiers_round_trip_through_sqlite() {
        let connection = Connection::open_in_memory().unwrap();
        init_processed_inputs_table(&connection).unwrap();
        let locator = "/nfs/nfcapd.202506010000";
        let revision = InputRevision::create("nfcapd", locator, "content", "decoder").unwrap();
        let snapshot = FileSnapshot {
            device: 59,
            inode: 12_920_913_336_376_042_522,
            size: 75_142_409,
            mtime_ns: 1_749_185_788_758_725_896,
            ctime_ns: 1_749_185_803_169_058_597,
        };
        upsert_input_bucket(
            &connection,
            &InputBucket {
                input_kind: InputKind::Nfcapd,
                input_locator: locator.into(),
                scan_locator: locator.into(),
                source_id: "r1".into(),
                bucket_start: 0,
                bucket_end: 300,
                revision: revision.clone(),
                file_snapshot: Some(snapshot.clone()),
            },
            false,
        )
        .unwrap();
        mark_input_bucket_status(
            &connection,
            InputKind::Nfcapd,
            locator,
            "r1",
            0,
            InputStatus::Processed,
            &revision,
            None,
        )
        .unwrap();
        assert_eq!(
            cached_content_fingerprint(&connection, InputKind::Nfcapd, locator, &snapshot)
                .unwrap()
                .as_deref(),
            Some("content")
        );
    }

    #[test]
    fn stats_rows_upsert_and_bucket_deletion_cover_every_table() {
        let connection = Connection::open_in_memory().unwrap();
        init_stats_tables(&connection).unwrap();
        insert_traffic_stats_rows(&connection, &[TrafficStatsRow::example()]).unwrap();
        insert_protocol_stats_rows(&connection, &[ProtocolStatsRow::example()]).unwrap();
        insert_address_count_stats_rows(&connection, &[AddressCountStatsRow::example()]).unwrap();
        insert_port_count_stats_rows(&connection, &[PortCountStatsRow::example()]).unwrap();
        insert_address_structure_stats_rows(&connection, &[AddressStructureStatsRow::example()])
            .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT flows_tcp FROM traffic_stats", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        delete_stats_bucket_keys(&connection, &[StatsBucketKey::new("r1", "5m", 0)]).unwrap();
        for table in STATS_TABLE_NAMES {
            let count = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    #[test]
    fn dataset_upsert_replaces_source_members() {
        let connection = Connection::open_in_memory().unwrap();
        upsert_dataset_metadata(
            &connection,
            &DatasetMetadata::new("d1").with_sources([SourceDefinition::new("r1", ["a", "b"])]),
        )
        .unwrap();
        upsert_dataset_metadata(
            &connection,
            &DatasetMetadata::new("d1").with_sources([SourceDefinition::new("r2", ["c"])]),
        )
        .unwrap();
        let members = connection
            .prepare(
                "SELECT source_id, member_id FROM source_members ORDER BY source_id, member_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(members, [("r2".into(), "c".into())]);
    }

    #[test]
    fn backup_captures_wal_and_promotion_preserves_existing_target() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("candidate.sqlite");
        let target = directory.path().join("target.sqlite");
        let previous = directory.path().join("previous.sqlite");
        let candidate_writer = connect_pipeline_writer(&candidate).unwrap();
        candidate_writer
            .execute("CREATE TABLE events (value TEXT)", [])
            .unwrap();
        candidate_writer
            .execute("INSERT INTO events VALUES ('new-in-wal')", [])
            .unwrap();
        let target_writer = connect_pipeline_writer(&target).unwrap();
        target_writer
            .execute("CREATE TABLE events (value TEXT)", [])
            .unwrap();
        target_writer
            .execute("INSERT INTO events VALUES ('old')", [])
            .unwrap();
        drop(target_writer);

        promote_database(&candidate, &target, Some(&previous)).unwrap();

        assert_eq!(
            Connection::open(&target)
                .unwrap()
                .query_row("SELECT value FROM events", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "new-in-wal"
        );
        assert_eq!(
            Connection::open(&previous)
                .unwrap()
                .query_row("SELECT value FROM events", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "old"
        );
        assert!(candidate.exists());
        assert!(!directory.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp")
        }));

        fs::write(target.with_file_name("target.sqlite-wal"), b"stale").unwrap();
        backup_database(&candidate, &target).unwrap();
        assert!(!target.with_file_name("target.sqlite-wal").exists());
    }

    #[test]
    fn promotion_rejects_backup_aliasing_candidate_or_target() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("candidate.sqlite");
        let target = directory.path().join("target.sqlite");
        drop(Connection::open(&candidate).unwrap());
        drop(Connection::open(&target).unwrap());

        for backup in [&candidate, &target] {
            let error = promote_database(&candidate, &target, Some(backup)).unwrap_err();
            assert!(error.to_string().contains("must be distinct"));
        }
    }

    #[test]
    fn maintenance_rejects_database_sidecar_and_lock_aliases_before_mutation() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("target.sqlite");
        for source in [
            sidecar_path(&target, "-wal"),
            sidecar_path(&target, "-shm"),
            sidecar_path(&target, "-journal"),
            database_operation_lock_path(&target).unwrap(),
        ] {
            drop(Connection::open(&source).unwrap());
            let original = fs::read(&source).unwrap();

            let error = backup_database(&source, &target).unwrap_err();

            assert!(error.to_string().contains("must be distinct"));
            assert_eq!(fs::read(&source).unwrap(), original);
            fs::remove_file(source).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn maintenance_resolves_symlinks_before_checking_related_paths() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target.sqlite");
        let lock = database_operation_lock_path(&target).unwrap();
        drop(Connection::open(&lock).unwrap());
        let candidate = directory.path().join("candidate.sqlite");
        symlink(&lock, &candidate).unwrap();
        let original = fs::read(&lock).unwrap();

        let error = backup_database(&candidate, &target).unwrap_err();

        assert!(error.to_string().contains("must be distinct"));
        assert_eq!(fs::read(lock).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn maintenance_rejects_a_derived_lock_symlink_to_the_source() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let target = directory.path().join("target.sqlite");
        drop(Connection::open(&source).unwrap());
        let lock = database_operation_lock_path(&target).unwrap();
        symlink(&source, &lock).unwrap();
        let original = fs::read(&source).unwrap();

        let error = backup_database(&source, &target).unwrap_err();

        assert!(error.to_string().contains("must be distinct"));
        assert_eq!(fs::read(source).unwrap(), original);
    }

    #[test]
    fn failed_transaction_rolls_back_all_persistence_changes() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute("CREATE TABLE events (value TEXT)", [])
            .unwrap();
        let result: Result<(), StorageError> = in_transaction(&mut connection, |transaction| {
            transaction.execute("INSERT INTO events VALUES ('uncommitted')", [])?;
            Err(StorageError::InvalidInput("stop".into()))
        });
        assert!(result.is_err());
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
