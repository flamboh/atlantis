//! Continuous Singularity alert feed over an nfcapd capture tree.
//!
//! `netflow-db feed <dataset>` is a long-running process: it polls the
//! dataset's capture tree for newly completed five-minute buckets, unions the
//! distinct addresses across the dataset's members for each window, scores
//! them with [`crate::singularity`], and appends threshold-crossing addresses
//! to a rolling alert database (`alerts.sqlite` beside the dataset's product
//! database). Rows older than the retention window are pruned on each pass.
//!
//! The alert database is an ephemeral rolling buffer owned by this module,
//! not a pipeline product database: it carries no dataset identity or
//! coverage semantics and is safe to delete at any time.

use std::{
    collections::BTreeSet,
    fs,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use jiff::Timestamp;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::{
    domain::{AddressSide, CanonicalBucket, FlowSelection, IpVersion, Scope, Visibility},
    ingest,
    registry::{self, DatasetRegistry},
    singularity,
};

const WINDOW_SECONDS: i64 = 300;
const GRACE_SECONDS: i64 = 10 * 60;
const SECONDS_PER_HOUR: i64 = 60 * 60;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

// This mirrors pipeline.rs's private DEFAULT_TIMEZONE. Registry datasets do
// not carry a timezone, and registry-driven pipeline runs use this default.
const TIMEZONE: &str = "America/Los_Angeles";

// Calibrated on 200 uOregon five-minute windows spanning 2025-06 through
// 2026-06 after conformance against the Haskell reference; a typical window
// records ~20 alerts (p10-p90: 14-25) across both tails at these values.
// Methodology and sensitivity: docs/agent/singularity-calibration.md.
const DEFAULT_THRESHOLD_HIGH: f64 = 2.0;
const DEFAULT_THRESHOLD_LOW: f64 = 0.3;

const ALERT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS feed_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS windows (
    window_start INTEGER PRIMARY KEY,
    window_end INTEGER NOT NULL,
    member_files INTEGER NOT NULL,
    address_count INTEGER NOT NULL,
    alert_count INTEGER NOT NULL,
    alpha_min REAL,
    alpha_max REAL,
    alpha_median REAL,
    processed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS alerts (
    window_start INTEGER NOT NULL REFERENCES windows(window_start) ON DELETE CASCADE,
    address TEXT NOT NULL,
    alpha REAL NOT NULL,
    tail TEXT NOT NULL CHECK (tail IN ('high','low')),
    rank INTEGER NOT NULL,
    r2 REAL NOT NULL,
    prefix_levels INTEGER NOT NULL,
    PRIMARY KEY (window_start, tail, rank)
);
CREATE INDEX IF NOT EXISTS alerts_address ON alerts(address, window_start);
"#;

/// Configuration for one `netflow-db feed` run.
#[derive(Debug)]
pub struct FeedOptions {
    /// Dataset id resolved through the datasets registry.
    pub dataset_id: String,
    /// Registry path override; defaults to standard `datasets.json` discovery.
    pub registry_path: Option<PathBuf>,
    /// Alert database path; defaults to `alerts.sqlite` beside the dataset's
    /// configured `db_path`.
    pub database_path: Option<PathBuf>,
    /// nfdump executable (the pinned fork supporting the atlantis contract).
    pub nfdump: String,
    /// Seconds between capture-tree scans.
    pub poll_seconds: u64,
    /// Days of alerts to retain.
    pub retention_days: u32,
    /// Cap on recorded alerts per tail per window.
    pub max_per_tail: u32,
    /// Alpha at or above which an address alerts; `None` uses the module default.
    pub threshold_high: Option<f64>,
    /// Alpha at or below which an address alerts; `None` uses the module default.
    pub threshold_low: Option<f64>,
    /// Also process historical windows this far back, e.g. `"36h"` or `"7d"`.
    pub backfill: Option<String>,
    /// Process available windows once and exit instead of polling.
    pub once: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error(transparent)]
    Registry(#[from] registry::RegistryError),
    #[error("alert database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("feed I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid feed configuration: {0}")]
    InvalidConfig(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlertTail {
    High,
    Low,
}

impl AlertTail {
    const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SelectedAlert {
    address: Ipv4Addr,
    alpha: f64,
    tail: AlertTail,
    rank: u32,
    r2: f64,
    prefix_levels: u8,
}

#[derive(Debug)]
struct MemberFile {
    member_id: String,
    path: PathBuf,
}

#[derive(Debug)]
struct WindowReadiness {
    existing_files: Vec<MemberFile>,
    processable: bool,
}

struct FeedContext<'a> {
    root_path: &'a Path,
    members: &'a [String],
    nfdump: &'a str,
    selection: &'a FlowSelection,
    threshold_high: f64,
    threshold_low: f64,
    max_per_tail: u32,
    retention_days: u32,
}

/// Run the feed until interrupted (or once, with [`FeedOptions::once`]).
pub fn run(options: FeedOptions) -> Result<(), FeedError> {
    let repository_root = std::env::current_dir()?;
    let registry = match &options.registry_path {
        Some(path) => DatasetRegistry::load(path, &repository_root)?,
        None => DatasetRegistry::load_default(&repository_root)?,
    };
    let dataset = registry.get(&options.dataset_id)?.clone();
    let members = dataset
        .logical_sources()?
        .into_iter()
        .flat_map(|source| source.members)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if members.is_empty() {
        return Err(FeedError::InvalidConfig(format!(
            "dataset {:?} has no capture members",
            dataset.dataset_id
        )));
    }

    let threshold_high = options.threshold_high.unwrap_or(DEFAULT_THRESHOLD_HIGH);
    let threshold_low = options.threshold_low.unwrap_or(DEFAULT_THRESHOLD_LOW);
    if !threshold_high.is_finite() || !threshold_low.is_finite() {
        return Err(FeedError::InvalidConfig(
            "alert thresholds must be finite numbers".into(),
        ));
    }
    let backfill_seconds = options
        .backfill
        .as_deref()
        .map(parse_backfill_duration)
        .transpose()?;

    let database_path = options.database_path.clone().unwrap_or_else(|| {
        dataset
            .db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("alerts.sqlite")
    });
    if let Some(parent) = database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut connection = open_alert_database(&database_path)?;
    upsert_feed_meta(
        &connection,
        &dataset.dataset_id,
        threshold_high,
        threshold_low,
        options.max_per_tail,
    )?;

    let startup_time = Timestamp::now().as_second();
    // Windows older than the retention cutoff would be pruned in the same
    // pass that processed them, so a backfill deeper than retention is
    // clamped rather than wasted.
    let retention_floor = retention_cutoff(startup_time, options.retention_days);
    let requested_start = backfill_seconds
        .map(|duration| startup_time.saturating_sub(duration))
        .unwrap_or(startup_time);
    if requested_start < retention_floor {
        tracing::warn!(
            retention_days = options.retention_days,
            "backfill exceeds retention; clamping scan start to the retention cutoff"
        );
    }
    let initial_scan_start = align_window_at_or_after(requested_start.max(retention_floor));
    let selection = FlowSelection::default();
    let context = FeedContext {
        root_path: &dataset.root_path,
        members: &members,
        nfdump: &options.nfdump,
        selection: &selection,
        threshold_high,
        threshold_low,
        max_per_tail: options.max_per_tail,
        retention_days: options.retention_days,
    };

    loop {
        let now = Timestamp::now().as_second();
        process_pass(&mut connection, &context, initial_scan_start, now)?;
        prune_windows(&connection, retention_cutoff(now, options.retention_days))?;

        if options.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(options.poll_seconds));
    }
}

fn open_alert_database(path: &Path) -> Result<Connection, FeedError> {
    let connection = Connection::open(path)?;
    init_alert_schema(&connection)?;
    Ok(connection)
}

fn init_alert_schema(connection: &Connection) -> Result<(), FeedError> {
    connection.busy_timeout(Duration::from_millis(crate::storage::BUSY_TIMEOUT_MS))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.execute_batch(ALERT_SCHEMA)?;
    Ok(())
}

fn upsert_feed_meta(
    connection: &Connection,
    dataset_id: &str,
    threshold_high: f64,
    threshold_low: f64,
    max_per_tail: u32,
) -> Result<(), FeedError> {
    const UPSERT: &str = "
        INSERT INTO feed_meta (key, value) VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
    ";
    for (key, value) in [
        ("schema_version", "1".to_owned()),
        ("dataset_id", dataset_id.to_owned()),
        ("threshold_high", threshold_high.to_string()),
        ("threshold_low", threshold_low.to_string()),
        ("max_per_tail", max_per_tail.to_string()),
    ] {
        connection.execute(UPSERT, params![key, value])?;
    }
    Ok(())
}

fn process_pass(
    connection: &mut Connection,
    context: &FeedContext<'_>,
    initial_scan_start: i64,
    now: i64,
) -> Result<(), FeedError> {
    let mut window_start =
        next_scan_start(connection, initial_scan_start, now, context.retention_days)?;

    loop {
        let window_end = window_start.checked_add(WINDOW_SECONDS).ok_or_else(|| {
            FeedError::InvalidConfig("candidate window timestamp overflowed".into())
        })?;
        if window_end > now {
            break;
        }
        if window_exists(connection, window_start)? {
            window_start = window_end;
            continue;
        }

        let readiness = window_readiness(context.root_path, context.members, window_start, now)?;
        if !readiness.processable {
            break;
        }

        let started_at = Instant::now();
        let mut member_files = 0_i64;
        let mut addresses = BTreeSet::new();
        for member_file in readiness.existing_files {
            match ingest::read_nfcapd_bucket(
                &member_file.path,
                &member_file.member_id,
                context.selection,
                context.nfdump,
                TIMEZONE,
            ) {
                Ok(bucket) => {
                    member_files += 1;
                    collect_total_ipv4_addresses(bucket, &mut addresses);
                }
                Err(error) => tracing::warn!(
                    path = %member_file.path.display(),
                    member = %member_file.member_id,
                    error = %error,
                    "failed to read feed capture file"
                ),
            }
        }

        if member_files == 0 {
            tracing::warn!(
                window_start,
                window_end,
                "no member files could be read for feed window"
            );
            window_start = window_end;
            continue;
        }

        let scores = singularity::score(addresses.into_iter().collect());
        let alerts = select_alerts(
            &scores,
            context.threshold_high,
            context.threshold_low,
            context.max_per_tail,
        );
        let (alpha_min, alpha_max, alpha_median) = alpha_summary(&scores);
        write_window(
            connection,
            &WindowRecord {
                window_start,
                window_end,
                member_files,
                scores: &scores,
                alerts: &alerts,
                alpha_min,
                alpha_max,
                alpha_median,
                processed_at: now,
            },
        )?;
        prune_windows(connection, retention_cutoff(now, context.retention_days))?;

        tracing::info!(
            window_start,
            window_end,
            address_count = scores.len(),
            alert_count = alerts.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "processed feed window"
        );
        window_start = window_end;
    }

    Ok(())
}

fn collect_total_ipv4_addresses(bucket: CanonicalBucket, addresses: &mut BTreeSet<Ipv4Addr>) {
    let total_ipv4_scope = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
    for scoped in bucket.addresses {
        if scoped.scope != total_ipv4_scope
            || !matches!(
                scoped.address_side,
                AddressSide::Source | AddressSide::Destination
            )
        {
            continue;
        }
        for address in scoped.addresses.iter() {
            if let IpAddr::V4(address) = address {
                addresses.insert(*address);
            }
        }
    }
}

/// One processed window's row data, written transactionally by [`write_window`].
struct WindowRecord<'a> {
    window_start: i64,
    window_end: i64,
    member_files: i64,
    scores: &'a [singularity::AddressScore],
    alerts: &'a [SelectedAlert],
    alpha_min: Option<f64>,
    alpha_max: Option<f64>,
    alpha_median: Option<f64>,
    processed_at: i64,
}

fn write_window(connection: &mut Connection, record: &WindowRecord) -> Result<(), FeedError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO windows (
            window_start, window_end, member_files, address_count, alert_count,
            alpha_min, alpha_max, alpha_median, processed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            record.window_start,
            record.window_end,
            record.member_files,
            record.scores.len() as i64,
            record.alerts.len() as i64,
            record.alpha_min,
            record.alpha_max,
            record.alpha_median,
            record.processed_at,
        ],
    )?;
    for alert in record.alerts {
        transaction.execute(
            "INSERT INTO alerts (
                window_start, address, alpha, tail, rank, r2, prefix_levels
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.window_start,
                alert.address.to_string(),
                alert.alpha,
                alert.tail.as_str(),
                i64::from(alert.rank),
                alert.r2,
                i64::from(alert.prefix_levels),
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn prune_windows(connection: &Connection, cutoff: i64) -> Result<(), FeedError> {
    connection.execute(
        "DELETE FROM windows WHERE window_start < ?1",
        params![cutoff],
    )?;
    Ok(())
}

fn window_readiness(
    root: &Path,
    members: &[String],
    window_start: i64,
    now: i64,
) -> Result<WindowReadiness, FeedError> {
    let mut existing_files = Vec::with_capacity(members.len());
    for member in members {
        let path = expected_nfcapd_path(root, member, window_start)?;
        if path.exists() {
            existing_files.push(MemberFile {
                member_id: member.clone(),
                path,
            });
        }
    }
    let all_present = existing_files.len() == members.len();
    let window_end = window_start
        .checked_add(WINDOW_SECONDS)
        .ok_or_else(|| FeedError::InvalidConfig("candidate window timestamp overflowed".into()))?;
    let past_grace = now.saturating_sub(window_end) > GRACE_SECONDS;
    Ok(WindowReadiness {
        existing_files,
        processable: all_present || past_grace,
    })
}

fn expected_nfcapd_path(
    root: &Path,
    member: &str,
    window_start: i64,
) -> Result<PathBuf, FeedError> {
    let timestamp = Timestamp::from_second(window_start)
        .and_then(|timestamp| timestamp.in_tz(TIMEZONE))
        .map_err(|error| {
            FeedError::InvalidConfig(format!(
                "invalid feed window timestamp {window_start}: {error}"
            ))
        })?;
    Ok(root
        .join(member)
        .join(timestamp.strftime("%Y").to_string())
        .join(timestamp.strftime("%m").to_string())
        .join(timestamp.strftime("%d").to_string())
        .join(format!("nfcapd.{}", timestamp.strftime("%Y%m%d%H%M"))))
}

fn next_scan_start(
    connection: &Connection,
    initial_scan_start: i64,
    now: i64,
    retention_days: u32,
) -> Result<i64, FeedError> {
    let newest = connection.query_row("SELECT MAX(window_start) FROM windows", [], |row| {
        row.get::<_, Option<i64>>(0)
    })?;
    match newest {
        Some(window_start) => {
            let after_newest = window_start.checked_add(WINDOW_SECONDS).ok_or_else(|| {
                FeedError::InvalidConfig("processed window timestamp overflowed".into())
            })?;
            Ok(after_newest.max(align_window_at_or_after(retention_cutoff(
                now,
                retention_days,
            ))))
        }
        None => Ok(initial_scan_start),
    }
}

fn window_exists(connection: &Connection, window_start: i64) -> Result<bool, FeedError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM windows WHERE window_start = ?1)",
        params![window_start],
        |row| row.get(0),
    )?)
}

fn retention_cutoff(now: i64, retention_days: u32) -> i64 {
    now.saturating_sub(i64::from(retention_days) * SECONDS_PER_DAY)
}

fn align_window_at_or_after(timestamp: i64) -> i64 {
    let remainder = timestamp.rem_euclid(WINDOW_SECONDS);
    if remainder == 0 {
        timestamp
    } else {
        timestamp.saturating_add(WINDOW_SECONDS - remainder)
    }
}

fn parse_backfill_duration(value: &str) -> Result<i64, FeedError> {
    let Some((unit_index, unit)) = value.char_indices().last() else {
        return Err(invalid_backfill(value));
    };
    let amount = &value[..unit_index];
    if amount.is_empty() || !amount.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_backfill(value));
    }
    let amount = amount.parse::<i64>().map_err(|_| invalid_backfill(value))?;
    let unit_seconds = match unit {
        'h' => SECONDS_PER_HOUR,
        'd' => SECONDS_PER_DAY,
        _ => return Err(invalid_backfill(value)),
    };
    amount.checked_mul(unit_seconds).ok_or_else(|| {
        FeedError::InvalidConfig(format!("backfill duration {value:?} is too large"))
    })
}

fn invalid_backfill(value: &str) -> FeedError {
    FeedError::InvalidConfig(format!(
        "invalid backfill duration {value:?}; expected an integer followed by 'h' or 'd'"
    ))
}

fn alpha_summary(scores: &[singularity::AddressScore]) -> (Option<f64>, Option<f64>, Option<f64>) {
    if scores.is_empty() {
        return (None, None, None);
    }
    let mut alphas = scores.iter().map(|score| score.alpha).collect::<Vec<_>>();
    alphas.sort_by(f64::total_cmp);
    let median_index = alphas.len() / 2;
    let median = if alphas.len() % 2 == 0 {
        alphas[median_index - 1] / 2.0 + alphas[median_index] / 2.0
    } else {
        alphas[median_index]
    };
    (
        alphas.first().copied(),
        alphas.last().copied(),
        Some(median),
    )
}

fn select_alerts(
    scores: &[singularity::AddressScore],
    threshold_high: f64,
    threshold_low: f64,
    max_per_tail: u32,
) -> Vec<SelectedAlert> {
    let mut high = scores
        .iter()
        .filter(|score| score.alpha >= threshold_high)
        .collect::<Vec<_>>();
    high.sort_by(|left, right| {
        right
            .alpha
            .total_cmp(&left.alpha)
            .then_with(|| left.address.cmp(&right.address))
    });

    let mut low = scores
        .iter()
        .filter(|score| score.alpha <= threshold_low)
        .collect::<Vec<_>>();
    low.sort_by(|left, right| {
        left.alpha
            .total_cmp(&right.alpha)
            .then_with(|| left.address.cmp(&right.address))
    });

    let limit = max_per_tail as usize;
    high.into_iter()
        .take(limit)
        .enumerate()
        .map(|(index, score)| selected_alert(score, AlertTail::High, index))
        .chain(
            low.into_iter()
                .take(limit)
                .enumerate()
                .map(|(index, score)| selected_alert(score, AlertTail::Low, index)),
        )
        .collect()
}

fn selected_alert(
    score: &singularity::AddressScore,
    tail: AlertTail,
    index: usize,
) -> SelectedAlert {
    SelectedAlert {
        address: score.address,
        alpha: score.alpha,
        tail,
        rank: index as u32 + 1,
        r2: score.r_squared,
        prefix_levels: score.prefix_levels,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn schema_initialization_is_idempotent() {
        let temporary = tempdir().unwrap();
        let database_path = temporary.path().join("alerts.sqlite");

        {
            let connection = Connection::open(&database_path).unwrap();
            init_alert_schema(&connection).unwrap();
            init_alert_schema(&connection).unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        init_alert_schema(&connection).unwrap();

        let tables = connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name IN ('feed_meta', 'windows', 'alerts')
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, vec!["alerts", "feed_meta", "windows"]);

        let alert_columns = connection
            .prepare("PRAGMA table_info(alerts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            alert_columns,
            vec![
                "window_start",
                "address",
                "alpha",
                "tail",
                "rank",
                "r2",
                "prefix_levels"
            ]
        );
    }

    #[test]
    fn pruning_removes_old_windows_and_their_alerts() {
        let temporary = tempdir().unwrap();
        let connection = open_alert_database(&temporary.path().join("alerts.sqlite")).unwrap();
        insert_window(&connection, 100);
        insert_window(&connection, 1_000);
        for window_start in [100, 1_000] {
            connection
                .execute(
                    "INSERT INTO alerts
                     (window_start, address, alpha, tail, rank, r2, prefix_levels)
                     VALUES (?1, '192.0.2.1', 4.0, 'high', 1, 0.9, 12)",
                    params![window_start],
                )
                .unwrap();
        }

        prune_windows(&connection, 1_000).unwrap();

        let windows = query_i64_column(&connection, "SELECT window_start FROM windows");
        let alerts = query_i64_column(&connection, "SELECT window_start FROM alerts");
        assert_eq!(windows, vec![1_000]);
        assert_eq!(alerts, vec![1_000]);
    }

    #[test]
    fn parses_hour_and_day_backfills() {
        assert_eq!(
            parse_backfill_duration("36h").unwrap(),
            36 * SECONDS_PER_HOUR
        );
        assert_eq!(parse_backfill_duration("7d").unwrap(), 7 * SECONDS_PER_DAY);
    }

    #[test]
    fn rejects_malformed_backfills() {
        for value in ["", "36", "h", "1.5h", "-2d", "7w", " 7d"] {
            assert!(
                parse_backfill_duration(value).is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn window_readiness_obeys_file_completeness_and_grace_period() {
        let temporary = tempdir().unwrap();
        let members = vec!["alpha".to_owned(), "beta".to_owned()];
        let complete_start = 1_700_000_100;
        for member in &members {
            create_empty_capture(temporary.path(), member, complete_start);
        }

        let complete = window_readiness(
            temporary.path(),
            &members,
            complete_start,
            complete_start + WINDOW_SECONDS,
        )
        .unwrap();
        assert!(complete.processable);
        assert_eq!(complete.existing_files.len(), 2);

        let partial_start = complete_start + WINDOW_SECONDS;
        create_empty_capture(temporary.path(), &members[0], partial_start);
        let before_grace = window_readiness(
            temporary.path(),
            &members,
            partial_start,
            partial_start + WINDOW_SECONDS + GRACE_SECONDS,
        )
        .unwrap();
        assert!(!before_grace.processable);
        assert_eq!(before_grace.existing_files.len(), 1);

        let after_grace = window_readiness(
            temporary.path(),
            &members,
            partial_start,
            partial_start + WINDOW_SECONDS + GRACE_SECONDS + 1,
        )
        .unwrap();
        assert!(after_grace.processable);
        assert_eq!(after_grace.existing_files.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn process_pass_skips_missing_window_and_processes_later_window() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("captures");
        let members = vec!["alpha".to_owned(), "beta".to_owned()];
        let missing_start = 1_700_000_100;
        let populated_start = missing_start + WINDOW_SECONDS;
        for member in &members {
            create_empty_capture(&root, member, populated_start);
        }

        let decoder = temporary.path().join("fake-nfdump");
        write_fake_nfdump(&decoder, "");
        let mut connection = open_alert_database(&temporary.path().join("alerts.sqlite")).unwrap();
        let selection = FlowSelection::default();
        let context = FeedContext {
            root_path: &root,
            members: &members,
            nfdump: decoder.to_str().unwrap(),
            selection: &selection,
            threshold_high: DEFAULT_THRESHOLD_HIGH,
            threshold_low: DEFAULT_THRESHOLD_LOW,
            max_per_tail: 0,
            retention_days: 1,
        };
        let now = missing_start + WINDOW_SECONDS + GRACE_SECONDS + 1;

        process_pass(&mut connection, &context, missing_start, now).unwrap();

        assert!(!window_exists(&connection, missing_start).unwrap());
        assert!(window_exists(&connection, populated_start).unwrap());
    }

    #[test]
    fn resume_starts_after_the_newest_window_and_respects_retention_floor() {
        let temporary = tempdir().unwrap();
        let connection = open_alert_database(&temporary.path().join("alerts.sqlite")).unwrap();
        insert_window(&connection, 900);

        assert_eq!(next_scan_start(&connection, 0, 2_000, 7).unwrap(), 1_200);

        connection.execute("DELETE FROM windows", []).unwrap();
        insert_window(&connection, 0);
        let now = 10 * SECONDS_PER_DAY;
        let retention_floor = align_window_at_or_after(now - SECONDS_PER_DAY);
        assert_eq!(
            next_scan_start(&connection, 0, now, 1).unwrap(),
            retention_floor
        );
    }

    #[test]
    fn alert_selection_caps_and_ranks_both_tails_deterministically() {
        let scores = vec![
            score_fixture([192, 0, 2, 4], 5.0),
            score_fixture([192, 0, 2, 2], 5.0),
            score_fixture([192, 0, 2, 3], 4.0),
            score_fixture([192, 0, 2, 9], -0.2),
            score_fixture([192, 0, 2, 7], 0.1),
            score_fixture([192, 0, 2, 8], 0.3),
            score_fixture([192, 0, 2, 6], 1.0),
        ];

        let alerts = select_alerts(&scores, 3.5, 0.4, 2);
        assert_eq!(alerts.len(), 4);
        assert_eq!(
            alerts
                .iter()
                .map(|alert| (alert.tail, alert.rank, alert.address, alert.alpha))
                .collect::<Vec<_>>(),
            vec![
                (AlertTail::High, 1, Ipv4Addr::new(192, 0, 2, 2), 5.0),
                (AlertTail::High, 2, Ipv4Addr::new(192, 0, 2, 4), 5.0),
                (AlertTail::Low, 1, Ipv4Addr::new(192, 0, 2, 9), -0.2),
                (AlertTail::Low, 2, Ipv4Addr::new(192, 0, 2, 7), 0.1),
            ]
        );
    }

    fn insert_window(connection: &Connection, window_start: i64) {
        connection
            .execute(
                "INSERT INTO windows (
                    window_start, window_end, member_files, address_count, alert_count,
                    alpha_min, alpha_max, alpha_median, processed_at
                 ) VALUES (?1, ?2, 1, 1, 1, 1.0, 1.0, 1.0, ?1)",
                params![window_start, window_start + WINDOW_SECONDS],
            )
            .unwrap();
    }

    fn query_i64_column(connection: &Connection, query: &str) -> Vec<i64> {
        connection
            .prepare(query)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn create_empty_capture(root: &Path, member: &str, window_start: i64) {
        let path = expected_nfcapd_path(root, member, window_start).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, []).unwrap();
    }

    #[cfg(unix)]
    fn write_fake_nfdump(executable: &std::path::Path, setup: &str) {
        use std::os::unix::fs::PermissionsExt;

        let stream = executable.with_extension("stream");
        fs::write(&stream, crate::nfdump::ONE_V4_TEST_STREAM).unwrap();
        fs::write(
            executable,
            format!("#!/bin/sh\n{setup}\ncat '{}'\n", stream.display()),
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn score_fixture(address: [u8; 4], alpha: f64) -> singularity::AddressScore {
        singularity::AddressScore {
            address: Ipv4Addr::from(address),
            alpha,
            intercept: 0.0,
            r_squared: 0.95,
            prefix_levels: 12,
        }
    }
}
