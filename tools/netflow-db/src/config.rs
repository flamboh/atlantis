use std::{collections::BTreeMap, fs, path::Path};

use jiff::civil::DateTime;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid timestamp value {0:?}")]
    InvalidTimestamp(String),
    #[error("unsupported timestamp format {0:?}")]
    UnsupportedTimestampFormat(String),
    #[error("invalid CSV configuration: {0}")]
    InvalidConfig(String),
    #[error("unable to read CSV configuration: {0}")]
    Io(#[from] std::io::Error),
    #[error("unable to parse CSV configuration: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Deserialize)]
struct RawSourceId {
    value: Option<String>,
    column: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RawArchive {
    member_contains: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RawDiscovery {
    include_contains: Option<Vec<String>>,
    include_suffixes: Option<Vec<String>>,
    exclude_suffixes: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawCsvSourceConfig {
    columns: BTreeMap<String, Option<String>>,
    source_id: Option<RawSourceId>,
    delimiter: Option<String>,
    has_header: Option<bool>,
    timestamp_format: Option<String>,
    datetime_format: Option<String>,
    timestamp_timezone: Option<String>,
    fieldnames: Option<Vec<String>>,
    protocol_map: Option<BTreeMap<String, i64>>,
    skip_bad_column_count: Option<bool>,
    archive: Option<RawArchive>,
    discovery: Option<RawDiscovery>,
    input_order: Option<String>,
    out_of_order_lag_buckets: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum InputOrder {
    TimestampAscending,
    Unsorted,
}

#[derive(Clone, Debug)]
pub struct CsvSourceConfig {
    pub delimiter: u8,
    pub has_header: bool,
    pub timestamp_format: String,
    pub datetime_format: String,
    pub timestamp_timezone: String,
    pub fieldnames: Option<Vec<String>>,
    pub columns: BTreeMap<String, String>,
    pub protocol_map: BTreeMap<String, i64>,
    pub source_id_value: Option<String>,
    pub source_id_column: Option<String>,
    pub skip_bad_column_count: bool,
    pub archive_member_contains: Option<String>,
    pub discovery_include_contains: Vec<String>,
    pub discovery_include_suffixes: Vec<String>,
    pub discovery_exclude_suffixes: Vec<String>,
    pub input_order: InputOrder,
    pub out_of_order_lag_buckets: u64,
}

impl CsvSourceConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let raw: RawCsvSourceConfig = serde_json::from_str(&contents)?;
        Self::try_from(raw)
    }

    pub fn parse_timestamp_ms(&self, raw: &str) -> Result<i64, ConfigError> {
        if self.timestamp_format == "datetime" {
            parse_datetime_timestamp_ms(raw.trim(), &self.timestamp_timezone, &self.datetime_format)
        } else {
            parse_numeric_timestamp_ms(raw, &self.timestamp_format)
        }
    }

    pub fn resolve_source_id<'a>(
        &self,
        value: impl Fn(&str) -> Option<&'a str>,
    ) -> Result<String, ConfigError> {
        if let Some(source_id) = &self.source_id_value {
            return Ok(source_id.clone());
        }
        let column = self
            .source_id_column
            .as_deref()
            .expect("validated source ID");
        value(column)
            .map(str::trim)
            .filter(|source_id| !source_id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                ConfigError::InvalidConfig(format!("missing source_id column {column:?}"))
            })
    }
}

impl TryFrom<RawCsvSourceConfig> for CsvSourceConfig {
    type Error = ConfigError;

    fn try_from(raw: RawCsvSourceConfig) -> Result<Self, Self::Error> {
        let columns: BTreeMap<_, _> = raw
            .columns
            .into_iter()
            .filter_map(|(key, value)| {
                value
                    .filter(|value| !value.is_empty())
                    .map(|value| (key, value))
            })
            .collect();
        if !["time_received", "time_end", "time_start"]
            .iter()
            .any(|key| columns.contains_key(*key))
        {
            return Err(ConfigError::InvalidConfig(
                "at least one timestamp column is required".into(),
            ));
        }
        for key in ["src_ip", "dst_ip"] {
            if !columns.contains_key(key) {
                return Err(ConfigError::InvalidConfig(format!(
                    "missing required column mapping {key:?}"
                )));
            }
        }

        let source_id_value = raw
            .source_id
            .as_ref()
            .and_then(|source| source.value.as_deref())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let source_id_column = raw
            .source_id
            .as_ref()
            .and_then(|source| source.column.as_deref())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if source_id_value.is_none() && source_id_column.is_none() {
            return Err(ConfigError::InvalidConfig(
                "source_id.value or source_id.column is required".into(),
            ));
        }

        let delimiter = raw.delimiter.unwrap_or_else(|| ",".into());
        if delimiter.len() != 1 || !delimiter.is_ascii() {
            return Err(ConfigError::InvalidConfig(
                "delimiter must be one single-byte character".into(),
            ));
        }
        let has_header = raw.has_header.unwrap_or(true);
        if !has_header && raw.fieldnames.is_none() {
            return Err(ConfigError::InvalidConfig(
                "fieldnames are required for headerless CSV".into(),
            ));
        }
        let timestamp_format = raw.timestamp_format.unwrap_or_else(|| "unix".into());
        if !matches!(timestamp_format.as_str(), "unix" | "unix_ms" | "datetime") {
            return Err(ConfigError::UnsupportedTimestampFormat(timestamp_format));
        }
        let timestamp_timezone = raw.timestamp_timezone.unwrap_or_else(|| "UTC".into());
        jiff::tz::TimeZone::get(&timestamp_timezone).map_err(|_| {
            ConfigError::InvalidConfig(format!("invalid IANA timezone {timestamp_timezone:?}"))
        })?;
        let datetime_format = raw
            .datetime_format
            .unwrap_or_else(|| "%Y-%m-%d %H:%M:%S".into());
        if datetime_format.is_empty() {
            return Err(ConfigError::InvalidConfig(
                "datetime_format cannot be empty".into(),
            ));
        }
        let input_order = match raw.input_order.as_deref().unwrap_or("timestamp_ascending") {
            "timestamp_ascending" => InputOrder::TimestampAscending,
            "unsorted" => InputOrder::Unsorted,
            other => {
                return Err(ConfigError::InvalidConfig(format!(
                    "unsupported input_order {other:?}"
                )));
            }
        };
        let archive = raw.archive.unwrap_or_default();
        let discovery = raw.discovery.unwrap_or_default();
        let mut protocol_map = default_protocol_map();
        protocol_map.extend(
            raw.protocol_map
                .unwrap_or_default()
                .into_iter()
                .map(|(name, number)| (name.to_ascii_uppercase(), number)),
        );

        Ok(Self {
            delimiter: delimiter.as_bytes()[0],
            has_header,
            timestamp_format,
            datetime_format,
            timestamp_timezone,
            fieldnames: raw.fieldnames,
            columns,
            protocol_map,
            source_id_value,
            source_id_column,
            skip_bad_column_count: raw.skip_bad_column_count.unwrap_or(false),
            archive_member_contains: archive.member_contains,
            discovery_include_contains: discovery
                .include_contains
                .unwrap_or_else(|| vec!["csv".into()]),
            discovery_include_suffixes: discovery
                .include_suffixes
                .unwrap_or_else(|| vec![".tar.gz".into(), ".tgz".into()]),
            discovery_exclude_suffixes: discovery
                .exclude_suffixes
                .unwrap_or_else(|| vec![".aria2".into(), ".txt".into()]),
            input_order,
            out_of_order_lag_buckets: raw.out_of_order_lag_buckets.unwrap_or(12),
        })
    }
}

/// Parse a numeric Unix timestamp into an exact signed 64-bit millisecond value.
pub fn parse_numeric_timestamp_ms(raw: &str, timestamp_format: &str) -> Result<i64, ConfigError> {
    let value = raw
        .trim()
        .parse::<Decimal>()
        .map_err(|_| ConfigError::InvalidTimestamp(raw.into()))?;
    let milliseconds = match timestamp_format {
        "unix" => value
            .checked_mul(Decimal::from(1_000))
            .ok_or_else(|| ConfigError::InvalidTimestamp(raw.into()))?,
        "unix_ms" => value,
        other => return Err(ConfigError::UnsupportedTimestampFormat(other.into())),
    };
    if milliseconds.fract().is_zero() {
        milliseconds
            .trunc()
            .to_string()
            .parse::<i64>()
            .map_err(|_| ConfigError::InvalidTimestamp(raw.into()))
    } else {
        Err(ConfigError::InvalidTimestamp(raw.into()))
    }
}

pub fn parse_datetime_timestamp_ms(
    raw: &str,
    timestamp_timezone: &str,
    datetime_format: &str,
) -> Result<i64, ConfigError> {
    let datetime = DateTime::strptime(datetime_format, raw)
        .map_err(|_| ConfigError::InvalidTimestamp(raw.into()))?;
    if datetime.subsec_nanosecond() % 1_000_000 != 0 {
        return Err(ConfigError::InvalidTimestamp(raw.into()));
    }
    let timestamp = datetime
        .in_tz(timestamp_timezone)
        .map_err(|_| ConfigError::InvalidTimestamp(raw.into()))?
        .timestamp();
    Ok(timestamp.as_millisecond())
}

pub fn floor_unix_timestamp(timestamp: i64, bucket_seconds: i64) -> Result<i64, ConfigError> {
    if bucket_seconds <= 0 {
        return Err(ConfigError::InvalidConfig(
            "bucket_seconds must be positive".into(),
        ));
    }
    Ok(timestamp - timestamp.rem_euclid(bucket_seconds))
}

fn default_protocol_map() -> BTreeMap<String, i64> {
    let mut protocols: BTreeMap<String, i64> = [
        ("ICMP", 1),
        ("IPIP", 4),
        ("TCP", 6),
        ("EGP", 8),
        ("UDP", 17),
        ("RSVP", 46),
        ("GRE", 47),
        ("ESP", 50),
        ("AH", 51),
        ("ICMPV6", 58),
        ("IPV6-ICMP", 58),
        ("EIGRP", 88),
        ("OSPF", 89),
        ("OSPFIGP", 89),
        ("PIM", 103),
        ("SCTP", 132),
    ]
    .into_iter()
    .map(|(name, number)| (name.into(), number))
    .collect();
    if let Ok(contents) = fs::read_to_string("/etc/protocols") {
        for line in contents.lines() {
            let fields = line
                .split('#')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>();
            let Some(number) = fields.get(1).and_then(|raw| raw.parse().ok()) else {
                continue;
            };
            for name in fields
                .first()
                .into_iter()
                .chain(fields.get(2..).unwrap_or_default())
            {
                protocols.insert(name.to_ascii_uppercase(), number);
            }
        }
    }
    protocols
}

#[cfg(test)]
mod tests {
    use super::{default_protocol_map, parse_numeric_timestamp_ms};

    #[test]
    fn numeric_timestamps_preserve_exact_milliseconds() {
        assert_eq!(
            parse_numeric_timestamp_ms("1744733279.999", "unix").unwrap(),
            1_744_733_279_999
        );
        assert!(parse_numeric_timestamp_ms("1.0001", "unix").is_err());
    }

    #[test]
    fn protocol_map_always_contains_the_canonical_fallbacks() {
        let protocols = default_protocol_map();
        assert_eq!(protocols.get("TCP"), Some(&6));
        assert_eq!(protocols.get("IPV6-ICMP"), Some(&58));
    }
}
