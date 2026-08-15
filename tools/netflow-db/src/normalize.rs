//! Input normalization shared by CSV files and `nfdump` subprocesses.

use std::{collections::BTreeMap, net::IpAddr, str::FromStr};

use rust_decimal::Decimal;
use thiserror::Error;

use crate::{
    config::{ConfigError, CsvSourceConfig},
    domain::{DomainError, FlowObservation},
};

pub const NFDUMP_CSV_FORMAT: &str =
    "csv:%trr,%ter,%tsr,%sa,%da,%sp,%dp,%pr,%pkt,%byt,%stos,%dtos,%fl,%minttl,%maxttl";
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

#[must_use]
pub fn build_nfdump_csv_command(
    executable: &str,
    file_path: &str,
    ip_version: u8,
) -> Option<Vec<String>> {
    let family = match ip_version {
        4 => vec!["ipv4"],
        6 => vec!["ipv6", "-6"],
        _ => return None,
    };
    Some(
        [executable, "-r", file_path, "-q", "-o", NFDUMP_CSV_FORMAT]
            .into_iter()
            .chain(family)
            .map(str::to_owned)
            .collect(),
    )
}

pub fn normalize_nfdump_values(
    values: &[String],
    source_id: &str,
) -> Result<NormalizedRow, NormalizeError> {
    if values.len() != 15 {
        return Err(NormalizeError::InvalidRow(format!(
            "nfdump CSV row must contain 15 values, got {}",
            values.len()
        )));
    }
    let timestamps = [
        parse_unix_ms(&values[0])?,
        parse_unix_ms(&values[1])?,
        parse_unix_ms(&values[2])?,
    ];
    let bucket_start = timestamps[0].div_euclid(300_000) * 300;
    let src_ip = parse_ip(&values[3])?;
    let dst_ip = parse_ip(&values[4])?;
    let protocol = bounded_u8(parse_integer(&values[7], "protocol")?, "protocol")?;
    let src_port = normalize_nfdump_port(&values[5], protocol)?;
    let dst_port = normalize_nfdump_port(&values[6], protocol)?;
    let packets = nonnegative_integer(&values[8], "packets")?;
    let bytes = nonnegative_integer(&values[9], "bytes")?;
    let src_tos = bounded_u8(parse_integer(&values[10], "src_tos")?, "src_tos")?;
    let dst_tos = bounded_u8(parse_integer(&values[11], "dst_tos")?, "dst_tos")?;
    let flow_count = positive_integer(&values[12], "flow_count")?;
    let min_ttl = normalize_nfdump_ttl(&values[13])?;
    let max_ttl = normalize_nfdump_ttl(&values[14])?;
    validate_ttl_order(min_ttl, max_ttl)?;

    let duration_ms = timestamps[1].checked_sub(timestamps[2]).ok_or_else(|| {
        NormalizeError::InvalidRow("flow duration exceeds signed 64-bit range".into())
    })?;
    if duration_ms < 0 {
        return Err(NormalizeError::InvalidRow(
            "flow time_end must not precede time_start".into(),
        ));
    }

    let mut observation = FlowObservation::new(src_ip, dst_ip, protocol, packets, bytes, src_tos)?
        .with_ports(src_port, dst_port)
        .with_measurements(Some(duration_ms), min_ttl, max_ttl)?
        .with_flow_count(flow_count)?;
    observation.time_received_ms = Some(timestamps[0]);
    observation.time_end_ms = Some(timestamps[1]);
    observation.time_start_ms = Some(timestamps[2]);
    observation.dst_tos = dst_tos;

    Ok(NormalizedRow {
        source_id: source_id.to_owned(),
        bucket_start,
        bucket_end: bucket_start + 300,
        observation,
    })
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

fn parse_unix_ms(raw: &str) -> Result<i64, NormalizeError> {
    let value = Decimal::from_str(raw.trim())
        .map_err(|_| NormalizeError::InvalidRow(format!("invalid Unix timestamp {raw:?}")))?;
    let milliseconds = value
        .checked_mul(Decimal::from(1_000))
        .filter(|value| value.fract().is_zero())
        .ok_or_else(|| {
            NormalizeError::InvalidRow(format!(
                "Unix timestamp {raw:?} must have millisecond precision"
            ))
        })?;
    milliseconds
        .trunc()
        .to_string()
        .parse()
        .map_err(|_| NormalizeError::InvalidRow(format!("invalid Unix timestamp {raw:?}")))
}

fn parse_ip(raw: &str) -> Result<IpAddr, NormalizeError> {
    raw.trim()
        .parse()
        .map_err(|_| NormalizeError::InvalidRow(format!("invalid IP address value {raw:?}")))
}

fn normalize_nfdump_port(raw: &str, protocol: u8) -> Result<Option<u16>, NormalizeError> {
    let raw = raw.trim();
    if !raw.contains('.') {
        return raw
            .parse()
            .map(Some)
            .map_err(|_| NormalizeError::InvalidRow(format!("invalid nfdump port {raw:?}")));
    }
    if !matches!(protocol, 1 | 58) {
        return Err(NormalizeError::InvalidRow(format!(
            "dotted nfdump pseudo-port {raw:?} is only valid for ICMP"
        )));
    }
    let components = raw.split('.').collect::<Vec<_>>();
    if components.len() != 2 || components.iter().any(|value| value.parse::<u8>().is_err()) {
        return Err(NormalizeError::InvalidRow(format!(
            "invalid nfdump ICMP type/code pseudo-port {raw:?}"
        )));
    }
    Ok(Some(0))
}

fn normalize_nfdump_ttl(raw: &str) -> Result<Option<u8>, NormalizeError> {
    match raw.trim() {
        "" | "0" => Ok(None),
        value => value
            .parse()
            .map(Some)
            .map_err(|_| NormalizeError::InvalidRow(format!("invalid nfdump TTL {raw:?}"))),
    }
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

fn positive_integer(raw: &str, name: &str) -> Result<i64, NormalizeError> {
    let value = nonnegative_integer(raw, name)?;
    if value == 0 {
        return Err(NormalizeError::InvalidRow(format!(
            "{name} must be at least 1"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfdump_adapter_preserves_missing_ttl_and_icmp_pseudo_ports() {
        let values = [
            "1744733279.999",
            "1744733279.999",
            "1744733279.000",
            "192.0.2.1",
            "198.51.100.1",
            "8.0",
            "0.0",
            "1",
            "2",
            "128",
            "0",
            "0",
            "3",
            "0",
            "64",
        ]
        .map(str::to_owned);

        let row = normalize_nfdump_values(&values, "router-a").unwrap();

        assert_eq!(row.bucket_start, 1_744_733_100);
        assert_eq!(row.observation.src_port, Some(0));
        assert_eq!(row.observation.min_ttl, None);
        assert_eq!(row.observation.max_ttl, Some(64));
        assert_eq!(row.observation.duration_ms, Some(999));
        assert_eq!(row.observation.flow_count, 3);
    }

    #[test]
    fn nfdump_command_uses_fixed_contract() {
        assert_eq!(
            build_nfdump_csv_command("nfdump", "/capture", 6).unwrap(),
            vec![
                "nfdump",
                "-r",
                "/capture",
                "-q",
                "-o",
                NFDUMP_CSV_FORMAT,
                "ipv6",
                "-6",
            ]
        );
    }
}
