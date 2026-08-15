//! Streaming reduction of nfdump's fixed 15-column CSV contract.

use fixedbitset::FixedBitSet;
use ipnet::IpNet;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;
use std::io::{BufRead, Write};
use std::net::IpAddr;

use crate::domain::{
    AddressSide, BucketKey, CanonicalBucket, ExactVisibility, FlowSelection, Granularity,
    IpVersion, Scope, ScopedAddresses, ScopedPorts, ScopedProtocols, ScopedTraffic, TrafficMetrics,
    Visibility as DomainVisibility,
};

pub const CONTRACT_VERSION: u32 = 1;
pub const INPUT_CONTRACT: &str = "nfdump-csv-15-v1";
pub const OUTPUT_CONTRACT: &str = "canonical-scopes-v1";
pub const VERSION_LINE: &str = "nfdump_reducer 1 nfdump-csv-15-v1 canonical-scopes-v1";
pub const CSV_HEADER: &str = "received,lastSeen,firstSeen,srcAddr,dstAddr,srcPort,dstPort,proto,packets,bytes,srcTos,dstTos,flows,minTTL,maxTTL";

const FIELD_COUNT: usize = 15;
const METRIC_COUNT: usize = 21;
const MAX_INTEGER: u64 = i64::MAX as u64;

const FLOWS: usize = 0;
const PACKETS: usize = 5;
const BYTES: usize = 10;
const DURATION_SUM_MS: usize = 15;
const DURATION_COUNT: usize = 16;
const MIN_TTL_SUM: usize = 17;
const MIN_TTL_COUNT: usize = 18;
const MAX_TTL_SUM: usize = 19;
const MAX_TTL_COUNT: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visibility {
    Literal,
    Anonymized,
}

impl Visibility {
    const fn matches_tos(self, anonymized: bool) -> bool {
        matches!(self, Self::Anonymized) == anonymized
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VisibilitySelection {
    pub source: Option<Visibility>,
    pub destination: Option<Visibility>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReducerResult {
    pub version: u32,
    pub input_contract: &'static str,
    pub output_contract: &'static str,
    pub scopes: Vec<ReducedScope>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReducedScope {
    pub ip_version: u8,
    pub src_visibility: &'static str,
    pub dst_visibility: &'static str,
    pub metrics: [u64; METRIC_COUNT],
    pub protocols: BTreeSet<String>,
    pub source_addresses: BTreeSet<String>,
    pub destination_addresses: BTreeSet<String>,
    pub source_ports_hex: String,
    pub destination_ports_hex: String,
}

#[derive(Debug)]
pub struct ReducerError {
    message: String,
}

impl ReducerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn at_line(line_number: u64, error: Self) -> Self {
        Self::new(format!("line {line_number}: {error}"))
    }
}

impl fmt::Display for ReducerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ReducerError {}

#[derive(Clone)]
struct ScopeAccumulator {
    ip_version: IpVersion,
    source_visibility: DomainVisibility,
    destination_visibility: DomainVisibility,
    metrics: [u64; METRIC_COUNT],
    protocols: [u64; 4],
    source_addresses: BTreeSet<IpAddr>,
    destination_addresses: BTreeSet<IpAddr>,
    source_ports: PortBitmap,
    destination_ports: PortBitmap,
}

impl ScopeAccumulator {
    fn new(
        ip_version: IpVersion,
        source_visibility: DomainVisibility,
        destination_visibility: DomainVisibility,
    ) -> Self {
        Self {
            ip_version,
            source_visibility,
            destination_visibility,
            metrics: [0; METRIC_COUNT],
            protocols: [0; 4],
            source_addresses: BTreeSet::new(),
            destination_addresses: BTreeSet::new(),
            source_ports: PortBitmap::default(),
            destination_ports: PortBitmap::default(),
        }
    }

    fn add_flow(&mut self, flow: &Flow) -> Result<(), ReducerError> {
        let offset = protocol_metric_offset(flow.protocol);
        checked_add(&mut self.metrics[FLOWS], flow.flow_count, "flows")?;
        checked_add(
            &mut self.metrics[FLOWS + offset],
            flow.flow_count,
            "protocol flows",
        )?;
        checked_add(&mut self.metrics[PACKETS], flow.packets, "packets")?;
        checked_add(
            &mut self.metrics[PACKETS + offset],
            flow.packets,
            "protocol packets",
        )?;
        checked_add(&mut self.metrics[BYTES], flow.bytes, "bytes")?;
        checked_add(
            &mut self.metrics[BYTES + offset],
            flow.bytes,
            "protocol bytes",
        )?;
        let duration_sum = checked_multiply(flow.duration_ms, flow.flow_count, "duration sum")?;
        checked_add(
            &mut self.metrics[DURATION_SUM_MS],
            duration_sum,
            "duration sum",
        )?;
        checked_add(
            &mut self.metrics[DURATION_COUNT],
            flow.flow_count,
            "duration count",
        )?;
        if let Some(min_ttl) = flow.min_ttl {
            let sum = checked_multiply(u64::from(min_ttl), flow.flow_count, "min TTL sum")?;
            checked_add(&mut self.metrics[MIN_TTL_SUM], sum, "min TTL sum")?;
            checked_add(
                &mut self.metrics[MIN_TTL_COUNT],
                flow.flow_count,
                "min TTL count",
            )?;
        }
        if let Some(max_ttl) = flow.max_ttl {
            let sum = checked_multiply(u64::from(max_ttl), flow.flow_count, "max TTL sum")?;
            checked_add(&mut self.metrics[MAX_TTL_SUM], sum, "max TTL sum")?;
            checked_add(
                &mut self.metrics[MAX_TTL_COUNT],
                flow.flow_count,
                "max TTL count",
            )?;
        }
        self.protocols[usize::from(flow.protocol) / 64] |=
            1_u64 << (usize::from(flow.protocol) % 64);
        self.source_addresses.insert(flow.source_address);
        self.destination_addresses.insert(flow.destination_address);
        self.source_ports.insert(flow.source_port);
        self.destination_ports.insert(flow.destination_port);
        Ok(())
    }

    fn finish(self) -> ReducedScope {
        let protocols = (0_u16..=255)
            .filter(|protocol| {
                let protocol = usize::from(*protocol);
                self.protocols[protocol / 64] & (1_u64 << (protocol % 64)) != 0
            })
            .map(|protocol| protocol.to_string())
            .collect();
        ReducedScope {
            ip_version: self.ip_version.number(),
            src_visibility: self.source_visibility.as_str(),
            dst_visibility: self.destination_visibility.as_str(),
            metrics: self.metrics,
            protocols,
            source_addresses: self
                .source_addresses
                .into_iter()
                .map(|address| address.to_string())
                .collect(),
            destination_addresses: self
                .destination_addresses
                .into_iter()
                .map(|address| address.to_string())
                .collect(),
            source_ports_hex: self.source_ports.to_hex(),
            destination_ports_hex: self.destination_ports.to_hex(),
        }
    }

    fn finish_bucket(
        self,
    ) -> (
        ScopedTraffic,
        ScopedProtocols,
        [ScopedAddresses; 2],
        [ScopedPorts; 2],
    ) {
        let scope = Scope::new(
            self.ip_version,
            self.source_visibility,
            self.destination_visibility,
        );
        let mut protocols = (0_u16..=255)
            .filter(|protocol| {
                let protocol = usize::from(*protocol);
                self.protocols[protocol / 64] & (1_u64 << (protocol % 64)) != 0
            })
            .map(|protocol| protocol.to_string())
            .collect::<Vec<_>>();
        // Canonical protocol lists are ordered as strings (for example,
        // "17" precedes "6"), matching StatisticalBucket's BTreeSet.
        protocols.sort_unstable();
        (
            ScopedTraffic {
                scope,
                metrics: traffic_metrics(self.metrics),
            },
            ScopedProtocols { scope, protocols },
            [
                ScopedAddresses {
                    scope,
                    address_side: AddressSide::Destination,
                    addresses: self.destination_addresses.into_iter().collect(),
                },
                ScopedAddresses {
                    scope,
                    address_side: AddressSide::Source,
                    addresses: self.source_addresses.into_iter().collect(),
                },
            ],
            [
                ScopedPorts {
                    scope,
                    port_side: AddressSide::Destination,
                    ports: self.destination_ports.into_inner(),
                },
                ScopedPorts {
                    scope,
                    port_side: AddressSide::Source,
                    ports: self.source_ports.into_inner(),
                },
            ],
        )
    }
}

#[derive(Clone, Default)]
struct PortBitmap(FixedBitSet);

impl PortBitmap {
    fn insert(&mut self, port: u16) {
        let port = usize::from(port);
        if self.0.len() < 65_536 {
            self.0.grow(65_536);
        }
        self.0.insert(port);
    }

    fn to_hex(&self) -> String {
        let words = self.0.as_slice();
        let Some(last_word) = words.iter().rposition(|word| *word != 0) else {
            return "0".to_owned();
        };
        let mut result = format!("{:x}", words[last_word]);
        let width = usize::BITS as usize / 4;
        for word in words[..last_word].iter().rev() {
            use fmt::Write as _;
            write!(result, "{word:0width$x}").expect("writing to a String cannot fail");
        }
        result
    }

    fn into_inner(self) -> FixedBitSet {
        self.0
    }
}

struct Flow {
    ip_version: IpVersion,
    protocol: u8,
    packets: u64,
    bytes: u64,
    flow_count: u64,
    duration_ms: u64,
    min_ttl: Option<u8>,
    max_ttl: Option<u8>,
    source_tos: u8,
    source_address: IpAddr,
    destination_address: IpAddr,
    source_port: u16,
    destination_port: u16,
}

/// Reduce nfdump stdout without materializing its rows.
pub fn reduce<R: BufRead>(
    input: R,
    selection: VisibilitySelection,
) -> Result<ReducerResult, ReducerError> {
    let scopes = reduce_scopes(input, selection, None)?;
    Ok(ReducerResult {
        version: CONTRACT_VERSION,
        input_contract: INPUT_CONTRACT,
        output_contract: OUTPUT_CONTRACT,
        scopes: scopes.into_iter().map(ScopeAccumulator::finish).collect(),
    })
}

/// Reduce nfdump stdout directly into the canonical five-minute bucket used by the pipeline.
pub fn reduce_to_bucket<R: BufRead>(
    input: R,
    key: BucketKey,
    selection: &FlowSelection,
) -> Result<CanonicalBucket, ReducerError> {
    let visibility = VisibilitySelection {
        source: selection.src_visibility().map(Visibility::from),
        destination: selection.dst_visibility().map(Visibility::from),
    };
    let scopes = reduce_scopes(input, visibility, selection.ip_prefix())?;
    let mut traffic = Vec::with_capacity(scopes.len());
    let mut protocols = Vec::with_capacity(scopes.len());
    let mut addresses = Vec::with_capacity(scopes.len() * 2);
    let mut ports = Vec::with_capacity(scopes.len() * 2);
    for accumulator in scopes {
        let (scope_traffic, scope_protocols, scope_addresses, scope_ports) =
            accumulator.finish_bucket();
        traffic.push(scope_traffic);
        protocols.push(scope_protocols);
        addresses.extend(scope_addresses);
        ports.extend(scope_ports);
    }
    let mut five_minute_starts = BTreeSet::new();
    if key.granularity == Granularity::FiveMinutes {
        five_minute_starts.insert(key.bucket_start);
    }
    Ok(CanonicalBucket {
        key,
        traffic,
        protocols,
        addresses,
        ports,
        five_minute_starts,
    })
}

fn reduce_scopes<R: BufRead>(
    mut input: R,
    selection: VisibilitySelection,
    ip_prefix: Option<&IpNet>,
) -> Result<Vec<ScopeAccumulator>, ReducerError> {
    let mut scopes = make_scopes();
    let mut line = String::new();
    let mut line_number = 0_u64;
    let mut saw_header = false;
    let mut saw_no_match = false;

    loop {
        line.clear();
        let bytes_read = input
            .read_line(&mut line)
            .map_err(|error| ReducerError::new(format!("failed to read reducer input: {error}")))?;
        if bytes_read == 0 {
            break;
        }
        line_number += 1;
        if line.ends_with('\n') {
            line.pop();
        }
        if line.ends_with('\r') {
            line.pop();
        }

        if !saw_header {
            if line != CSV_HEADER {
                return Err(ReducerError::new("line 1: unexpected CSV header"));
            }
            saw_header = true;
            continue;
        }
        if line == "No matching flows" {
            if line_number != 2 {
                return Err(ReducerError::new(
                    "No matching flows must be the only data row",
                ));
            }
            saw_no_match = true;
            continue;
        }
        if line.is_empty() {
            return Err(ReducerError::new(format!(
                "line {line_number}: empty CSV row"
            )));
        }
        if saw_no_match {
            return Err(ReducerError::new("data row follows No matching flows"));
        }

        let flow = parse_flow(&line).map_err(|error| ReducerError::at_line(line_number, error))?;
        if !allows_visibility(flow.source_tos, selection)
            || ip_prefix.is_some_and(|prefix| {
                !prefix.contains(&flow.source_address)
                    && !prefix.contains(&flow.destination_address)
            })
        {
            continue;
        }
        let family_base = if flow.ip_version == IpVersion::V4 {
            0
        } else {
            5
        };
        for index in [
            family_base,
            family_base + exact_scope_index(flow.source_tos),
        ] {
            scopes[index]
                .add_flow(&flow)
                .map_err(|error| ReducerError::at_line(line_number, error))?;
        }
    }

    if !saw_header {
        return Err(ReducerError::new("missing CSV header"));
    }
    Ok(scopes)
}

/// Reduce nfdump stdout and write the canonical JSON contract directly.
pub fn reduce_to_json<R: BufRead, W: Write>(
    input: R,
    selection: VisibilitySelection,
    mut output: W,
) -> Result<(), ReducerError> {
    let result = reduce(input, selection)?;
    serde_json::to_writer(&mut output, &result)
        .map_err(|error| ReducerError::new(format!("failed to write reducer output: {error}")))?;
    output
        .write_all(b"\n")
        .map_err(|error| ReducerError::new(format!("failed to write reducer output: {error}")))
}

fn make_scopes() -> Vec<ScopeAccumulator> {
    const VISIBILITIES: [(DomainVisibility, DomainVisibility); 5] = [
        (DomainVisibility::All, DomainVisibility::All),
        (DomainVisibility::Anonymized, DomainVisibility::Anonymized),
        (DomainVisibility::Anonymized, DomainVisibility::Literal),
        (DomainVisibility::Literal, DomainVisibility::Anonymized),
        (DomainVisibility::Literal, DomainVisibility::Literal),
    ];
    [IpVersion::V4, IpVersion::V6]
        .into_iter()
        .flat_map(|family| {
            VISIBILITIES
                .map(|(source, destination)| ScopeAccumulator::new(family, source, destination))
        })
        .collect()
}

fn parse_flow(line: &str) -> Result<Flow, ReducerError> {
    let fields = split_csv_line(line)?;
    parse_unix_milliseconds(fields[0], "time_received")?;
    let end = parse_unix_milliseconds(fields[1], "time_end")?;
    let start = parse_unix_milliseconds(fields[2], "time_start")?;
    let duration_ms = duration_milliseconds(start, end)?;
    let source_ip: IpAddr = fields[3]
        .parse()
        .map_err(|_| ReducerError::new("invalid IP address"))?;
    let destination_ip: IpAddr = fields[4]
        .parse()
        .map_err(|_| ReducerError::new("invalid IP address"))?;
    let ip_version = IpVersion::of(source_ip);
    if source_ip.is_ipv4() != destination_ip.is_ipv4() {
        return Err(ReducerError::new("mixed IP families"));
    }
    let protocol = parse_unsigned(fields[7], "protocol", 255)? as u8;
    let source_port = parse_port(fields[5], protocol)?;
    let destination_port = parse_port(fields[6], protocol)?;
    let packets = parse_unsigned(fields[8], "packets", MAX_INTEGER)?;
    let bytes = parse_unsigned(fields[9], "bytes", MAX_INTEGER)?;
    let source_tos = parse_unsigned(fields[10], "src_tos", 255)? as u8;
    parse_unsigned(fields[11], "dst_tos", 255)?;
    let flow_count = parse_unsigned(fields[12], "flow_count", MAX_INTEGER)?;
    if flow_count == 0 {
        return Err(ReducerError::new("flow_count must be positive"));
    }
    let min_ttl = parse_optional_ttl(fields[13], "min_ttl")?;
    let max_ttl = parse_optional_ttl(fields[14], "max_ttl")?;
    if min_ttl.zip(max_ttl).is_some_and(|(min, max)| min > max) {
        return Err(ReducerError::new("min_ttl exceeds max_ttl"));
    }
    Ok(Flow {
        ip_version,
        protocol,
        packets,
        bytes,
        flow_count,
        duration_ms,
        min_ttl,
        max_ttl,
        source_tos,
        source_address: source_ip,
        destination_address: destination_ip,
        source_port,
        destination_port,
    })
}

fn split_csv_line(line: &str) -> Result<[&str; FIELD_COUNT], ReducerError> {
    let mut fields = [""; FIELD_COUNT];
    let mut values = line.split(',');
    for field in &mut fields {
        let Some(value) = values.next() else {
            return Err(ReducerError::new("CSV row has too few fields"));
        };
        *field = value;
    }
    if values.next().is_some() {
        Err(ReducerError::new("CSV row has too many fields"))
    } else {
        Ok(fields)
    }
}

impl From<ExactVisibility> for Visibility {
    fn from(value: ExactVisibility) -> Self {
        match value {
            ExactVisibility::Literal => Self::Literal,
            ExactVisibility::Anonymized => Self::Anonymized,
        }
    }
}

fn traffic_metrics(metrics: [u64; METRIC_COUNT]) -> TrafficMetrics {
    let [
        flows,
        flows_tcp,
        flows_udp,
        flows_icmp,
        flows_other,
        packets,
        packets_tcp,
        packets_udp,
        packets_icmp,
        packets_other,
        bytes,
        bytes_tcp,
        bytes_udp,
        bytes_icmp,
        bytes_other,
        duration_sum_ms,
        duration_count,
        min_ttl_sum,
        min_ttl_count,
        max_ttl_sum,
        max_ttl_count,
    ] = metrics;
    TrafficMetrics {
        flows: flows as i64,
        flows_tcp: flows_tcp as i64,
        flows_udp: flows_udp as i64,
        flows_icmp: flows_icmp as i64,
        flows_other: flows_other as i64,
        packets: packets as i64,
        packets_tcp: packets_tcp as i64,
        packets_udp: packets_udp as i64,
        packets_icmp: packets_icmp as i64,
        packets_other: packets_other as i64,
        bytes: bytes as i64,
        bytes_tcp: bytes_tcp as i64,
        bytes_udp: bytes_udp as i64,
        bytes_icmp: bytes_icmp as i64,
        bytes_other: bytes_other as i64,
        duration_sum_ms: duration_sum_ms as i64,
        duration_count: duration_count as i64,
        min_ttl_sum: min_ttl_sum as i64,
        min_ttl_count: min_ttl_count as i64,
        max_ttl_sum: max_ttl_sum as i64,
        max_ttl_count: max_ttl_count as i64,
    }
}

fn parse_unsigned(value: &str, name: &str, maximum: u64) -> Result<u64, ReducerError> {
    if value.is_empty() {
        return Err(ReducerError::new(format!("{name} is empty")));
    }
    let mut result = 0_u64;
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return Err(ReducerError::new(format!(
                "{name} is not an unsigned integer"
            )));
        }
        let digit = u64::from(byte - b'0');
        if result > (maximum - digit) / 10 {
            return Err(ReducerError::new(format!("{name} is out of range")));
        }
        result = result * 10 + digit;
    }
    Ok(result)
}

fn parse_unix_milliseconds(value: &str, name: &str) -> Result<i64, ReducerError> {
    if value.is_empty() {
        return Err(ReducerError::new(format!("{name} is empty")));
    }
    let (negative, magnitude) = value
        .strip_prefix('-')
        .map_or((false, value), |value| (true, value));
    let (seconds_text, fraction) = magnitude
        .split_once('.')
        .map_or((magnitude, ""), |parts| parts);
    if seconds_text.is_empty() || fraction.len() > 3 {
        return Err(ReducerError::new(format!(
            "{name} must have millisecond precision"
        )));
    }
    let maximum_magnitude = if negative {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    let seconds = parse_unsigned(seconds_text, name, maximum_magnitude / 1000)?;
    let mut milliseconds = if fraction.is_empty() {
        0
    } else {
        parse_unsigned(fraction, name, 999)?
    };
    for _ in fraction.len()..3 {
        milliseconds *= 10;
    }
    let absolute = seconds * 1000 + milliseconds;
    if absolute > maximum_magnitude {
        return Err(ReducerError::new(format!("{name} is out of range")));
    }
    if negative {
        if absolute == (i64::MAX as u64) + 1 {
            Ok(i64::MIN)
        } else {
            Ok(-(absolute as i64))
        }
    } else {
        Ok(absolute as i64)
    }
}

fn duration_milliseconds(start: i64, end: i64) -> Result<u64, ReducerError> {
    if end < start {
        return Err(ReducerError::new("time_end precedes time_start"));
    }
    let duration = i128::from(end) - i128::from(start);
    if duration > i128::from(i64::MAX) {
        return Err(ReducerError::new("duration exceeds signed 64-bit range"));
    }
    Ok(duration as u64)
}

fn parse_port(value: &str, protocol: u8) -> Result<u16, ReducerError> {
    if value.contains('.') {
        if protocol != 1 && protocol != 58 {
            return Err(ReducerError::new(
                "dotted pseudo-port is only valid for ICMP or ICMPv6",
            ));
        }
        let Some((kind, code)) = value.split_once('.') else {
            unreachable!("contains('.') guarantees split_once('.')")
        };
        if kind.is_empty() || code.is_empty() || code.contains('.') {
            return Err(ReducerError::new("invalid ICMP type/code pseudo-port"));
        }
        parse_unsigned(kind, "ICMP type", 255)?;
        parse_unsigned(code, "ICMP code", 255)?;
        return Ok(0);
    }
    Ok(parse_unsigned(value, "port", 65_535)? as u16)
}

fn parse_optional_ttl(value: &str, name: &str) -> Result<Option<u8>, ReducerError> {
    if value.is_empty() || value == "0" {
        Ok(None)
    } else {
        Ok(Some(parse_unsigned(value, name, 255)? as u8))
    }
}

fn allows_visibility(tos: u8, selection: VisibilitySelection) -> bool {
    selection
        .source
        .is_none_or(|visibility| visibility.matches_tos(tos & 2 != 0))
        && selection
            .destination
            .is_none_or(|visibility| visibility.matches_tos(tos & 1 != 0))
}

fn exact_scope_index(tos: u8) -> usize {
    [4, 3, 2, 1][usize::from(tos & 3)]
}

const fn protocol_metric_offset(protocol: u8) -> usize {
    match protocol {
        6 => 1,
        17 => 2,
        1 | 58 => 3,
        _ => 4,
    }
}

fn checked_add(target: &mut u64, value: u64, name: &str) -> Result<(), ReducerError> {
    if value > MAX_INTEGER || *target > MAX_INTEGER - value {
        return Err(ReducerError::new(format!(
            "{name} exceeds signed 64-bit range"
        )));
    }
    *target += value;
    Ok(())
}

fn checked_multiply(left: u64, right: u64, name: &str) -> Result<u64, ReducerError> {
    if left != 0 && right > MAX_INTEGER / left {
        return Err(ReducerError::new(format!(
            "{name} exceeds signed 64-bit range"
        )));
    }
    Ok(left * right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn csv(rows: &[&str]) -> String {
        let mut input = format!("{CSV_HEADER}\n");
        for row in rows {
            input.push_str(row);
            input.push('\n');
        }
        input
    }

    fn reduce_text(input: impl AsRef<[u8]>) -> Result<ReducerResult, ReducerError> {
        reduce(Cursor::new(input), VisibilitySelection::default())
    }

    #[test]
    fn empty_capture_returns_all_ten_dense_scopes() {
        let result = reduce_text(csv(&[])).unwrap();

        assert_eq!(result.scopes.len(), 10);
        assert!(result.scopes.iter().all(|scope| scope.metrics == [0; 21]));
        assert_eq!(result.scopes[0].source_ports_hex, "0");
        assert_eq!(result.scopes[9].ip_version, 6);
    }

    #[test]
    fn aggregates_all_metrics_protocols_addresses_ports_and_scopes() {
        let input = csv(&[
            "0.000,1.500,0.000,192.0.2.1,198.51.100.1,0,1023,6,10,1000,0,0,2,31,64",
            "0.000,2.000,1.000,192.0.2.1,198.51.100.2,1024,65535,17,20,2000,1,255,3,0,0",
            "0.000,0.000,0.000,2001:db8::1,2001:db8::2,8.0,3.1,58,30,3000,2,0,1,1,255",
            "0.000,4.250,4.000,2001:db8::3,2001:db8::4,443,53,132,40,4000,3,1,1,10,20",
        ]);

        let result = reduce_text(input).unwrap();

        assert_eq!(
            result.scopes[0].metrics,
            [
                5, 2, 3, 0, 0, 30, 10, 20, 0, 0, 3000, 1000, 2000, 0, 0, 6000, 5, 62, 2, 128, 2,
            ]
        );
        assert_eq!(
            result.scopes[0].protocols,
            BTreeSet::from(["17".into(), "6".into()])
        );
        assert_eq!(
            result.scopes[0].source_ports_hex,
            format!("1{}1", "0".repeat(255))
        );
        assert_eq!(
            result.scopes[0].destination_ports_hex,
            format!("8{}8{}", "0".repeat(16_127), "0".repeat(255))
        );
        assert_eq!(result.scopes[5].metrics[0], 2);
        assert_eq!(result.scopes[6].metrics[0], 1);
        assert_eq!(result.scopes[7].metrics[0], 1);
    }

    #[test]
    fn visibility_selection_filters_before_aggregation() {
        let input = csv(&[
            "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,1,0,1,20,30",
            "0.000,1.000,0.000,192.0.2.2,198.51.100.2,3,4,17,1,10,0,0,1,20,30",
        ]);

        let result = reduce(
            Cursor::new(input),
            VisibilitySelection {
                source: Some(Visibility::Literal),
                destination: Some(Visibility::Anonymized),
            },
        )
        .unwrap();

        assert_eq!(result.scopes[0].metrics[0], 1);
        assert_eq!(result.scopes[3].metrics[0], 1);
        assert_eq!(result.scopes[4].metrics[0], 0);
    }

    #[test]
    fn reduces_directly_to_a_selected_canonical_bucket() {
        let input = csv(&[
            "0.000,1.000,0.000,192.0.2.1,198.51.100.1,443,55000,6,10,1000,1,0,2,31,64",
            "0.000,1.000,0.000,203.0.113.1,198.51.100.2,53,55001,17,20,2000,1,0,3,32,63",
            "0.000,1.000,0.000,192.0.2.2,198.51.100.3,80,55002,6,30,3000,0,0,4,33,62",
        ]);
        let selection = FlowSelection::from_payload(Some(&serde_json::json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": "192.0.2.0/24",
            "src_visibility": "literal",
            "dst_visibility": "anonymized",
        })))
        .unwrap();
        let key = BucketKey::new(
            "edge-a",
            Granularity::FiveMinutes,
            1_700_000_000,
            1_700_000_300,
        );

        let bucket = reduce_to_bucket(Cursor::new(input), key.clone(), &selection).unwrap();

        assert_eq!(bucket.key, key);
        assert_eq!(bucket.five_minute_starts, BTreeSet::from([1_700_000_000]));
        assert_eq!(bucket.traffic.len(), 10);
        assert_eq!(bucket.protocols.len(), 10);
        assert_eq!(bucket.addresses.len(), 20);
        assert_eq!(bucket.ports.len(), 20);
        let all_scope = Scope::new(IpVersion::V4, DomainVisibility::All, DomainVisibility::All);
        let exact_scope = Scope::new(
            IpVersion::V4,
            DomainVisibility::Literal,
            DomainVisibility::Anonymized,
        );
        for scope in [all_scope, exact_scope] {
            let metrics = &bucket
                .traffic
                .iter()
                .find(|entry| entry.scope == scope)
                .unwrap()
                .metrics;
            assert_eq!(metrics.flows, 2);
            assert_eq!(metrics.flows_tcp, 2);
            assert_eq!(metrics.packets, 10);
            assert_eq!(metrics.bytes, 1000);
            assert_eq!(metrics.duration_sum_ms, 2000);
            assert_eq!(metrics.duration_count, 2);
            assert_eq!(metrics.min_ttl_sum, 62);
            assert_eq!(metrics.max_ttl_sum, 128);
        }
        assert_eq!(
            bucket
                .addresses
                .iter()
                .find(|entry| {
                    entry.scope == all_scope && entry.address_side == AddressSide::Source
                })
                .unwrap()
                .addresses,
            ["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        assert!(
            bucket
                .ports
                .iter()
                .find(|entry| entry.scope == all_scope && entry.port_side == AddressSide::Source)
                .unwrap()
                .ports
                .contains(443)
        );
    }

    #[test]
    fn canonical_bucket_protocols_keep_textual_order() {
        let input = csv(&[
            "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,20,30",
            "0.000,1.000,0.000,192.0.2.2,198.51.100.2,1,2,17,1,10,0,0,1,20,30",
        ]);
        let bucket = reduce_to_bucket(
            Cursor::new(input),
            BucketKey::new("edge-a", Granularity::FiveMinutes, 0, 300),
            &FlowSelection::default(),
        )
        .unwrap();
        let all_v4 = Scope::new(IpVersion::V4, DomainVisibility::All, DomainVisibility::All);

        assert_eq!(
            bucket
                .protocols
                .iter()
                .find(|entry| entry.scope == all_v4)
                .unwrap()
                .protocols,
            vec!["17".to_owned(), "6".to_owned()]
        );
    }

    #[test]
    fn icmp_pseudo_ports_collapse_to_zero() {
        let input = csv(&[
            "0.000,1.000,0.000,192.0.2.1,198.51.100.1,8.0,3.1,1,1,10,0,0,1,20,30",
            "0.000,1.000,0.000,2001:db8::1,2001:db8::2,128.0,1.4,58,1,10,0,0,1,20,30",
        ]);

        let result = reduce_text(input).unwrap();

        assert_eq!(result.scopes[0].source_ports_hex, "1");
        assert_eq!(result.scopes[0].destination_ports_hex, "1");
        assert_eq!(result.scopes[5].source_ports_hex, "1");
        assert_eq!(result.scopes[5].destination_ports_hex, "1");
    }

    #[test]
    fn rejects_invalid_rows_and_pseudo_ports_with_line_number() {
        let invalid = [
            ("1,2,malformed", "CSV row has too few fields"),
            (
                "0.000,1.000,0.000,192.0.2.1,2001:db8::1,1,2,6,1,10,0,0,1,20,30",
                "mixed IP families",
            ),
            (
                "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,3.1,6,1,10,0,0,1,20,30",
                "dotted pseudo-port",
            ),
            (
                "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,256.1,1,1,10,0,0,1,20,30",
                "ICMP type is out of range",
            ),
            (
                "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,0,20,30",
                "flow_count must be positive",
            ),
            (
                "0.000,1.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,31,30",
                "min_ttl exceeds max_ttl",
            ),
        ];
        for (row, expected) in invalid {
            let error = reduce_text(csv(&[row])).unwrap_err().to_string();
            assert!(error.starts_with("line 2:"), "{error}");
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn timestamp_boundaries_and_overflow_match_the_contract() {
        for timestamp in ["-9223372036854775.808", "9223372036854775.807", "-0.001"] {
            let row = format!(
                "0.000,{timestamp},{timestamp},192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2"
            );
            assert_eq!(reduce_text(csv(&[&row])).unwrap().scopes[0].metrics[15], 0);
        }

        for (row, expected) in [
            (
                "0.000,9223372036854775.808,9223372036854775.808,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2",
                "out of range",
            ),
            (
                "0.000,1.0000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2",
                "millisecond precision",
            ),
            (
                "0.000,2.000,0.000,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,9223372036854775807,1,2",
                "duration sum exceeds",
            ),
            (
                "0.000,9223372036854775.807,-9223372036854775.808,192.0.2.1,198.51.100.1,1,2,6,1,10,0,0,1,1,2",
                "duration exceeds",
            ),
        ] {
            assert!(
                reduce_text(csv(&[row]))
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }
    }

    #[test]
    fn requires_exact_stream_framing() {
        let cases = [
            ("".to_owned(), "missing CSV header"),
            ("wrong,header\n".to_owned(), "unexpected CSV header"),
            (format!("{CSV_HEADER}\n\n"), "empty CSV row"),
            (
                csv(&["No matching flows", "No matching flows"]),
                "only data row",
            ),
            (
                csv(&[
                    "No matching flows",
                    "0,0,0,192.0.2.1,198.51.100.1,1,2,6,1,1,0,0,1,1,1",
                ]),
                "data row follows",
            ),
        ];
        for (input, expected) in cases {
            assert!(
                reduce_text(input)
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }
        assert_eq!(
            reduce_text(csv(&["No matching flows"]))
                .unwrap()
                .scopes
                .len(),
            10
        );
    }

    #[test]
    fn writes_the_versioned_json_contract() {
        let mut output = Vec::new();
        reduce_to_json(
            Cursor::new(csv(&[])),
            VisibilitySelection::default(),
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.starts_with(
            "{\"version\":1,\"input_contract\":\"nfdump-csv-15-v1\",\"output_contract\":\"canonical-scopes-v1\",\"scopes\":["
        ));
        assert!(output.ends_with("]}\n"));
    }
}
