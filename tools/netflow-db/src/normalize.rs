//! Input normalization for configured CSV sources.

use std::{collections::BTreeMap, net::IpAddr, str::FromStr};

use rust_decimal::Decimal;
use thiserror::Error;

use crate::{
    config::{ConfigError, CsvSourceConfig},
    domain::{DomainError, FlowObservation},
};

const MAX_SQLITE_INTEGER: i64 = i64::MAX;
const TIMESTAMP_KEYS: [&str; 3] = ["time_received", "time_end", "time_start"];

#[derive(Debug, Error)]
pub enum NormalizeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("{0}")]
    InvalidRow(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRow {
    pub source_id: String,
    pub bucket_start: i64,
    pub bucket_end: i64,
    pub observation: FlowObservation,
}

pub fn field_indexes(
    headers: &[String],
    config: &CsvSourceConfig,
) -> Result<BTreeMap<String, usize>, NormalizeError> {
    let by_name: BTreeMap<_, _> = headers
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();
    let mut indexes = BTreeMap::new();
    for name in config.columns.values().chain(&config.source_id_column) {
        let index = by_name.get(name).copied().ok_or_else(|| {
            NormalizeError::InvalidRow(format!("CSV header is missing configured column {name:?}"))
        })?;
        indexes.insert(name.clone(), index);
    }
    Ok(indexes)
}

pub fn normalize_csv_values(
    values: &[String],
    config: &CsvSourceConfig,
    indexes: &BTreeMap<String, usize>,
) -> Result<NormalizedRow, NormalizeError> {
    let value = |name: &str| -> Result<&str, NormalizeError> {
        let index = indexes.get(name).ok_or_else(|| {
            NormalizeError::InvalidRow(format!("missing field index for {name:?}"))
        })?;
        values.get(*index).map(String::as_str).ok_or_else(|| {
            NormalizeError::InvalidRow(format!("CSV row is missing column {name:?}"))
        })
    };
    let optional = |logical: &str| -> Result<Option<&str>, NormalizeError> {
        config
            .columns
            .get(logical)
            .map(|column| value(column))
            .transpose()
            .map(|result| result.filter(|raw| !raw.trim().is_empty()))
    };

    let source_id = if let Some(source) = &config.source_id_value {
        source.clone()
    } else {
        let column = config
            .source_id_column
            .as_deref()
            .expect("source ID validated by configuration");
        required(value(column)?, column)?.to_owned()
    };
    let mut timestamps = BTreeMap::new();
    for key in TIMESTAMP_KEYS {
        if let Some(raw) = optional(key)? {
            timestamps.insert(key, config.parse_timestamp_ms(raw)?);
        }
    }
    let timestamp = TIMESTAMP_KEYS
        .into_iter()
        .find_map(|key| timestamps.get(key).copied())
        .ok_or_else(|| {
            NormalizeError::InvalidRow(
                "CSV row did not contain a usable configured timestamp".into(),
            )
        })?;
    let bucket_start = timestamp.div_euclid(300_000) * 300;

    let src_column = &config.columns["src_ip"];
    let dst_column = &config.columns["dst_ip"];
    let src_ip = parse_ip(required(value(src_column)?, src_column)?)?;
    let dst_ip = parse_ip(required(value(dst_column)?, dst_column)?)?;
    let protocol = extract_protocol(optional("protocol")?, config)?;
    let packets = extract_nonnegative(optional("packets")?, "packets", 0)?;
    let bytes = extract_nonnegative(optional("bytes")?, "bytes", 0)?;
    let src_tos = extract_u8(optional("src_tos")?, "src_tos", 0)?;
    let dst_tos = extract_u8(optional("dst_tos")?, "dst_tos", 0)?;
    let src_port = extract_u16(optional("src_port")?, "src_port")?;
    let dst_port = extract_u16(optional("dst_port")?, "dst_port")?;
    let min_ttl = extract_optional_u8(optional("min_ttl")?, "min_ttl")?;
    let max_ttl = extract_optional_u8(optional("max_ttl")?, "max_ttl")?;
    validate_ttl_order(min_ttl, max_ttl)?;
    let flow_count = extract_nonnegative(optional("flow_count")?, "flow_count", 1)?;
    if flow_count == 0 {
        return Err(NormalizeError::InvalidRow(
            "flow_count must be at least 1".into(),
        ));
    }
    let duration_ms = resolve_duration(optional("duration")?, &timestamps)?;

    let mut observation = FlowObservation::new(src_ip, dst_ip, protocol, packets, bytes, src_tos)?
        .with_ports(src_port, dst_port)
        .with_measurements(duration_ms, min_ttl, max_ttl)?
        .with_flow_count(flow_count)?;
    observation.time_received_ms = timestamps.get("time_received").copied();
    observation.time_end_ms = timestamps.get("time_end").copied();
    observation.time_start_ms = timestamps.get("time_start").copied();
    observation.dst_tos = dst_tos;

    Ok(NormalizedRow {
        source_id,
        bucket_start,
        bucket_end: bucket_start + 300,
        observation,
    })
}

fn parse_ip(raw: &str) -> Result<IpAddr, NormalizeError> {
    raw.trim()
        .parse()
        .map_err(|_| NormalizeError::InvalidRow(format!("invalid IP address value {raw:?}")))
}

fn required<'a>(raw: &'a str, column: &str) -> Result<&'a str, NormalizeError> {
    let value = raw.trim();
    if value.is_empty() {
        Err(NormalizeError::InvalidRow(format!(
            "CSV row is missing required value for column {column:?}"
        )))
    } else {
        Ok(value)
    }
}

fn parse_integer(raw: &str, name: &str) -> Result<i64, NormalizeError> {
    raw.trim().parse().map_err(|_| {
        NormalizeError::InvalidRow(format!("invalid integer value {raw:?} for {name}"))
    })
}

fn nonnegative_integer(raw: &str, name: &str) -> Result<i64, NormalizeError> {
    let value = parse_integer(raw, name)?;
    if !(0..=MAX_SQLITE_INTEGER).contains(&value) {
        return Err(NormalizeError::InvalidRow(format!(
            "{name} must be a nonnegative signed 64-bit integer"
        )));
    }
    Ok(value)
}

fn bounded_u8(value: i64, name: &str) -> Result<u8, NormalizeError> {
    value
        .try_into()
        .map_err(|_| NormalizeError::InvalidRow(format!("{name} must be in the range 0..255")))
}

fn extract_protocol(raw: Option<&str>, config: &CsvSourceConfig) -> Result<u8, NormalizeError> {
    let Some(raw) = raw else { return Ok(0) };
    if let Some(value) = config.protocol_map.get(&raw.trim().to_ascii_uppercase()) {
        return bounded_u8(*value, "protocol");
    }
    bounded_u8(parse_integer(raw, "protocol")?, "protocol")
}

fn extract_nonnegative(raw: Option<&str>, name: &str, default: i64) -> Result<i64, NormalizeError> {
    raw.map_or(Ok(default), |value| nonnegative_integer(value, name))
}

fn extract_u8(raw: Option<&str>, name: &str, default: u8) -> Result<u8, NormalizeError> {
    raw.map_or(Ok(default), |value| {
        bounded_u8(parse_integer(value, name)?, name)
    })
}

fn extract_optional_u8(raw: Option<&str>, name: &str) -> Result<Option<u8>, NormalizeError> {
    raw.map(|value| bounded_u8(parse_integer(value, name)?, name))
        .transpose()
}

fn extract_u16(raw: Option<&str>, name: &str) -> Result<Option<u16>, NormalizeError> {
    raw.map(|value| {
        let value = parse_integer(value, name)?;
        value.try_into().map_err(|_| {
            NormalizeError::InvalidRow(format!("{name} must be in the range 0..65535"))
        })
    })
    .transpose()
}

fn resolve_duration(
    explicit_seconds: Option<&str>,
    timestamps: &BTreeMap<&str, i64>,
) -> Result<Option<i64>, NormalizeError> {
    let start = timestamps.get("time_start").copied();
    let end = timestamps.get("time_end").copied();
    if matches!((start, end), (Some(start), Some(end)) if end < start) {
        return Err(NormalizeError::InvalidRow(
            "flow time_end must not precede time_start".into(),
        ));
    }
    if let Some(raw) = explicit_seconds {
        let seconds = Decimal::from_str(raw.trim())
            .map_err(|_| NormalizeError::InvalidRow(format!("invalid duration value {raw:?}")))?;
        let milliseconds = seconds
            .checked_mul(Decimal::from(1_000))
            .filter(|value| value.fract().is_zero() && !value.is_sign_negative())
            .ok_or_else(|| {
                NormalizeError::InvalidRow(format!(
                    "duration {raw:?} must be nonnegative seconds with millisecond precision"
                ))
            })?;
        return milliseconds
            .trunc()
            .to_string()
            .parse()
            .map(Some)
            .map_err(|_| NormalizeError::InvalidRow(format!("invalid duration value {raw:?}")));
    }
    match (start, end) {
        (Some(start), Some(end)) => end
            .checked_sub(start)
            .map(Some)
            .ok_or_else(|| NormalizeError::InvalidRow("derived duration overflow".into())),
        _ => Ok(None),
    }
}

fn validate_ttl_order(minimum: Option<u8>, maximum: Option<u8>) -> Result<(), NormalizeError> {
    if matches!((minimum, maximum), (Some(minimum), Some(maximum)) if minimum > maximum) {
        Err(NormalizeError::InvalidRow(
            "min_ttl must be less than or equal to max_ttl".into(),
        ))
    } else {
        Ok(())
    }
}
