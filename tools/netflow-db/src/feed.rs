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

use std::path::PathBuf;

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
    /// Alpha at or above which an address alerts; `None` uses the calibrated
    /// default.
    pub threshold_high: Option<f64>,
    /// Alpha at or below which an address alerts; `None` uses the calibrated
    /// default.
    pub threshold_low: Option<f64>,
    /// Also process historical windows this far back, e.g. `"36h"` or `"7d"`.
    pub backfill: Option<String>,
    /// Process available windows once and exit instead of polling.
    pub once: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("feed failure: {0}")]
    Other(String),
}

/// Run the feed until interrupted (or once, with [`FeedOptions::once`]).
pub fn run(options: FeedOptions) -> Result<(), FeedError> {
    let _ = options;
    todo!("feed loop: implemented by the feed task")
}
