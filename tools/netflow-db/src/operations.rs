//! Small operational integrations kept outside the core pipeline.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

use regex::Regex;
use reqwest::blocking::Client;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum OperationsError {
    #[error("HTTP operation failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database not found: {0}")]
    DatabaseNotFound(PathBuf),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UgrAssetKind {
    Csv,
    Nfcapd,
}

/// Scrape the UGR16 index hierarchy and return deterministic absolute asset URLs.
pub fn scrape_ugr16_urls(
    base_url: &str,
    kind: UgrAssetKind,
    months: &BTreeSet<String>,
) -> Result<Vec<String>, OperationsError> {
    let base = Url::parse(base_url)?;
    let client = Client::builder().timeout(Duration::from_secs(60)).build()?;
    let href = Regex::new(r#"href="([a-z]+\.php)#INI"#).expect("static regex");
    let week_href = Regex::new(r#"href="([a-z]+_week\d+\.php)#INI"#).expect("static regex");
    let hidden = Regex::new(r#"<input id="[^"]*hidden" type="hidden" value="([^"]+)""#)
        .expect("static regex");
    let comments = Regex::new(r"(?s)<!--.*?-->").expect("static regex");
    let fetch = |path: &str| -> Result<String, OperationsError> {
        Ok(client
            .get(base.join(path)?)
            .send()?
            .error_for_status()?
            .text()?)
    };
    let index = comments.replace_all(&fetch("index.php")?, "").into_owned();
    let month_pages = href
        .captures_iter(&index)
        .map(|capture| capture[1].to_owned())
        .collect::<BTreeSet<_>>();
    let mut week_pages = BTreeSet::new();
    for month_page in month_pages {
        let page = comments.replace_all(&fetch(&month_page)?, "").into_owned();
        week_pages.extend(
            week_href
                .captures_iter(&page)
                .map(|capture| capture[1].to_owned()),
        );
    }
    let normalized_months = months
        .iter()
        .map(|month| month.trim().to_ascii_lowercase())
        .filter(|month| !month.is_empty())
        .collect::<BTreeSet<_>>();
    let csv_name = Regex::new(r"^[a-z]+_week\d+_csv\.tar\.gz$").expect("static regex");
    let mut urls = BTreeSet::new();
    for week_page in week_pages {
        let page = comments.replace_all(&fetch(&week_page)?, "").into_owned();
        for capture in hidden.captures_iter(&page) {
            let path = &capture[1];
            let filename = path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let matches = match kind {
                UgrAssetKind::Csv => csv_name.is_match(&filename),
                UgrAssetKind::Nfcapd => path.to_ascii_lowercase().contains("nfcapd"),
            };
            let month = filename.split('_').next().unwrap_or_default();
            if matches && (normalized_months.is_empty() || normalized_months.contains(month)) {
                urls.insert(base.join(path)?.to_string());
            }
        }
    }
    Ok(urls.into_iter().collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerificationWindow {
    pub start: i64,
    pub end: i64,
    pub detail_bucket: i64,
}

pub fn select_web_verification_window(
    database: &Path,
    source_id: &str,
) -> Result<VerificationWindow, OperationsError> {
    if !database.is_file() {
        return Err(OperationsError::DatabaseNotFound(database.to_owned()));
    }
    let connection = Connection::open(database)?;
    let detail_bucket = connection
        .query_row(
            "
            SELECT ts.bucket_start
            FROM traffic_stats ts
            JOIN address_structure_stats st
              ON st.source_id = ts.source_id AND st.granularity = '5m'
             AND st.bucket_start = ts.bucket_start AND st.ip_version = 4
             AND st.src_visibility = 'all' AND st.dst_visibility = 'all'
             AND st.structure_kind = 'structure'
            JOIN address_structure_stats sp
              ON sp.source_id = ts.source_id AND sp.granularity = '5m'
             AND sp.bucket_start = ts.bucket_start AND sp.ip_version = 4
             AND sp.src_visibility = 'all' AND sp.dst_visibility = 'all'
             AND sp.structure_kind = 'spectrum'
            WHERE ts.source_id = ?1 AND ts.granularity = '5m'
              AND ts.src_visibility = 'all' AND ts.dst_visibility = 'all'
            ORDER BY ts.bucket_start LIMIT 1
            ",
            [source_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(|| {
            OperationsError::Invalid(format!(
                "no five-minute MAAD detail bucket for source {source_id:?}"
            ))
        })?;
    Ok(VerificationWindow {
        start: detail_bucket,
        end: detail_bucket + 3_600,
        detail_bucket,
    })
}

pub fn verify_web_routes(
    base_url: &str,
    dataset: &str,
    source_id: &str,
    window: VerificationWindow,
) -> Result<(), OperationsError> {
    let base = Url::parse(base_url)?;
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let datasets = request_json(&client, &base, "/api/datasets", &[])?;
    let summaries = datasets
        .as_array()
        .or_else(|| datasets.get("data").and_then(Value::as_array))
        .or_else(|| datasets.get("datasets").and_then(Value::as_array))
        .ok_or_else(|| OperationsError::Invalid("datasets API returned no list".into()))?;
    if !summaries
        .iter()
        .any(|entry| entry.get("datasetId").and_then(Value::as_str) == Some(dataset))
    {
        return Err(OperationsError::Invalid(format!(
            "datasets API did not include {dataset:?}"
        )));
    }
    let start = window.start.to_string();
    let end = window.end.to_string();
    let common = [
        ("dataset", dataset),
        ("routers", source_id),
        ("startDate", &start),
        ("endDate", &end),
    ];
    let query_with = |name: &'static str, value: &'static str| {
        common
            .into_iter()
            .chain([(name, value)])
            .collect::<Vec<_>>()
    };
    assert_nonempty(
        request_json(
            &client,
            &base,
            "/api/netflow/stats",
            &query_with("groupBy", "hour"),
        )?,
        "result",
        "/api/netflow/stats",
    )?;
    for route in ["/api/ip/stats", "/api/protocol/stats"] {
        assert_nonempty(
            request_json(&client, &base, route, &query_with("granularity", "1h"))?,
            "buckets",
            route,
        )?;
    }
    for route in [
        "/api/netflow/structure-stats",
        "/api/netflow/spectrum-stats",
    ] {
        assert_nonempty(
            request_json(&client, &base, route, &query_with("granularity", "5m"))?,
            "buckets",
            route,
        )?;
    }
    let slug = jiff::Timestamp::new(window.detail_bucket, 0)
        .and_then(|timestamp| timestamp.in_tz("America/Los_Angeles"))
        .map_err(|error| OperationsError::Invalid(error.to_string()))?
        .strftime("%Y%m%d%H%M")
        .to_string();
    assert_nonempty(
        request_json(
            &client,
            &base,
            &format!("/api/netflow/files/{slug}/details"),
            &[("dataset", dataset)],
        )?,
        "routers",
        "/api/netflow/files/[slug]/details",
    )?;
    Ok(())
}

fn request_json(
    client: &Client,
    base: &Url,
    route: &str,
    query: &[(&str, &str)],
) -> Result<Value, OperationsError> {
    Ok(client
        .get(base.join(route.trim_start_matches('/'))?)
        .query(query)
        .send()?
        .error_for_status()?
        .json()?)
}

fn assert_nonempty(payload: Value, key: &str, route: &str) -> Result<(), OperationsError> {
    if let Some(error) = payload.get("error") {
        return Err(OperationsError::Invalid(format!(
            "{route} returned error: {error}"
        )));
    }
    if payload
        .get(key)
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(OperationsError::Invalid(format!(
            "{route} returned empty {key}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_kind_is_explicit() {
        assert_ne!(UgrAssetKind::Csv, UgrAssetKind::Nfcapd);
    }

    #[test]
    fn verification_window_has_one_hour_width() {
        let window = VerificationWindow {
            start: 300,
            end: 3_900,
            detail_bucket: 300,
        };
        assert_eq!(window.end - window.start, 3_600);
    }
}
