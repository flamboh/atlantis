//! Private decoder for the Atlantis Flow Stream emitted by the nfdump fork.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use fixedbitset::FixedBitSet;

use crate::{
    coverage::BucketCoverage,
    domain::{
        AddressSet, AddressSide, BucketKey, CanonicalBucket, ExactVisibility, FlowSelection,
        Granularity, IpVersion, Scope, ScopedAddresses, ScopedPorts, ScopedProtocols,
        ScopedTraffic, TrafficMetrics, Visibility,
    },
};

pub(crate) const OUTPUT_MODE: &str = "atlantis";
pub(crate) const CONTRACT_VERSION: u32 = 1;
pub(crate) const INPUT_CONTRACT: &str = "atlantis-flow-stream-v1";
pub(crate) const OUTPUT_CONTRACT: &str = "canonical-scopes-v1";

const MAGIC: &[u8; 8] = b"ATLNFLOW";
const RECORD_LEN: usize = 72;
const MAX_BLOCK_RECORDS: usize = 2_048;
const MAX_BLOCK_BYTES: usize = MAX_BLOCK_RECORDS * RECORD_LEN;
const METRIC_COUNT: usize = 21;

const FLOWS: usize = 0;
const PACKETS: usize = 5;
const BYTES: usize = 10;
const DURATION_SUM_MS: usize = 15;
const DURATION_COUNT: usize = 16;
const MIN_TTL_SUM: usize = 17;
const MIN_TTL_COUNT: usize = 18;
const MAX_TTL_SUM: usize = 19;
const MAX_TTL_COUNT: usize = 20;

#[cfg(test)]
pub(crate) const ONE_V4_TEST_STREAM: &[u8] = &[
    65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 1, 0, 0, 0, 192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 198, 51, 100, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 128,
    0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 232, 3, 0, 0, 0, 0, 0, 0, 187, 1, 216, 214, 6, 0,
    31, 64, 0, 0, 0, 0,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Header,
    BlockHeader,
    BlockPayload,
    Record,
    Aggregate,
    End,
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Header => "header",
            Self::BlockHeader => "block header",
            Self::BlockPayload => "block payload",
            Self::Record => "record",
            Self::Aggregate => "aggregate",
            Self::End => "end marker",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Magic,
    Version,
    RecordLength,
    BlockCount,
    SourceAddress,
    DestinationAddress,
    Packets,
    Bytes,
    FlowCount,
    Duration,
    SourcePort,
    DestinationPort,
    Protocol,
    Tag,
    MinTtl,
    MaxTtl,
    StreamEnd,
}

impl fmt::Display for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Magic => "magic",
            Self::Version => "version",
            Self::RecordLength => "record_len",
            Self::BlockCount => "block_count",
            Self::SourceAddress => "src_addr",
            Self::DestinationAddress => "dst_addr",
            Self::Packets => "packets",
            Self::Bytes => "bytes",
            Self::FlowCount => "flow_count",
            Self::Duration => "duration_ms",
            Self::SourcePort => "src_port",
            Self::DestinationPort => "dst_port",
            Self::Protocol => "protocol",
            Self::Tag => "tag",
            Self::MinTtl => "min_ttl",
            Self::MaxTtl => "max_ttl",
            Self::StreamEnd => "stream_end",
        })
    }
}

#[derive(Debug)]
enum ErrorReason {
    Io(io::Error),
    ShortRead { expected: usize, actual: usize },
    UnexpectedMagic,
    UnsupportedVersion(u16),
    UnexpectedRecordLength(u16),
    ExcessiveBlockCount(u32),
    MissingTerminator,
    TrailingBytes,
    InvalidTag(u8),
    NonzeroIpv4Padding,
    NumericOverflow(u64),
    ZeroFlowCount,
    NonzeroIcmpDestinationPort(u16),
    InvalidTtlOrder { minimum: u8, maximum: u8 },
    AggregateOverflow,
}

impl fmt::Display for ErrorReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "read failed: {error}"),
            Self::ShortRead { expected, actual } => {
                write!(
                    formatter,
                    "short read: expected {expected} bytes, received {actual}"
                )
            }
            Self::UnexpectedMagic => formatter.write_str("expected ATLNFLOW magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported contract version {version}")
            }
            Self::UnexpectedRecordLength(length) => {
                write!(
                    formatter,
                    "record length is {length}, expected {RECORD_LEN}"
                )
            }
            Self::ExcessiveBlockCount(count) => {
                write!(formatter, "block count {count} exceeds {MAX_BLOCK_RECORDS}")
            }
            Self::MissingTerminator => formatter.write_str("missing mandatory end marker"),
            Self::TrailingBytes => formatter.write_str("bytes follow the mandatory end marker"),
            Self::InvalidTag(tag) => write!(formatter, "tag {tag:#04x} has reserved bits set"),
            Self::NonzeroIpv4Padding => {
                formatter.write_str("IPv4 address has nonzero trailing padding")
            }
            Self::NumericOverflow(value) => {
                write!(formatter, "value {value} exceeds signed 64-bit range")
            }
            Self::ZeroFlowCount => formatter.write_str("flow_count must be positive"),
            Self::NonzeroIcmpDestinationPort(port) => {
                write!(
                    formatter,
                    "ICMP destination port must be zero, received {port}"
                )
            }
            Self::InvalidTtlOrder { minimum, maximum } => {
                write!(
                    formatter,
                    "minimum TTL {minimum} exceeds maximum TTL {maximum}"
                )
            }
            Self::AggregateOverflow => formatter.write_str("exceeds signed 64-bit aggregate range"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct NfdumpError {
    phase: Phase,
    block_index: Option<u64>,
    record_ordinal: Option<u64>,
    field: Field,
    reason: ErrorReason,
}

impl NfdumpError {
    fn new(phase: Phase, field: Field, reason: ErrorReason) -> Self {
        Self {
            phase,
            block_index: None,
            record_ordinal: None,
            field,
            reason,
        }
    }

    fn at_block(mut self, block_index: u64) -> Self {
        self.block_index = Some(block_index);
        self
    }

    fn at_record(mut self, record_ordinal: u64) -> Self {
        self.record_ordinal = Some(record_ordinal);
        self
    }
}

impl fmt::Display for NfdumpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Atlantis Flow Stream v1 {} error", self.phase)?;
        if let Some(block_index) = self.block_index {
            write!(formatter, " at block {block_index}")?;
        }
        if let Some(record_ordinal) = self.record_ordinal {
            write!(formatter, ", record {record_ordinal}")?;
        }
        write!(formatter, ", field {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for NfdumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.reason {
            ErrorReason::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Default)]
struct ScopeAccumulator {
    metrics: [i64; METRIC_COUNT],
    protocols: [u64; 4],
    source_addresses: AddressSet,
    destination_addresses: AddressSet,
    source_ports: FixedBitSet,
    destination_ports: FixedBitSet,
}

impl ScopeAccumulator {
    fn validate_add(&self, flow: &Flow) -> Result<(), Field> {
        let protocol_offset = protocol_metric_offset(flow.protocol);
        self.validate_metric(FLOWS, flow.flow_count, Field::FlowCount)?;
        self.validate_metric(FLOWS + protocol_offset, flow.flow_count, Field::FlowCount)?;
        self.validate_metric(PACKETS, flow.packets, Field::Packets)?;
        self.validate_metric(PACKETS + protocol_offset, flow.packets, Field::Packets)?;
        self.validate_metric(BYTES, flow.bytes, Field::Bytes)?;
        self.validate_metric(BYTES + protocol_offset, flow.bytes, Field::Bytes)?;
        self.validate_metric(DURATION_SUM_MS, flow.duration_sum_ms, Field::Duration)?;
        self.validate_metric(DURATION_COUNT, flow.flow_count, Field::FlowCount)?;
        if let Some(sum) = flow.min_ttl_sum {
            self.validate_metric(MIN_TTL_SUM, sum, Field::MinTtl)?;
            self.validate_metric(MIN_TTL_COUNT, flow.flow_count, Field::FlowCount)?;
        }
        if let Some(sum) = flow.max_ttl_sum {
            self.validate_metric(MAX_TTL_SUM, sum, Field::MaxTtl)?;
            self.validate_metric(MAX_TTL_COUNT, flow.flow_count, Field::FlowCount)?;
        }
        Ok(())
    }

    fn validate_metric(&self, index: usize, value: i64, field: Field) -> Result<(), Field> {
        self.metrics[index]
            .checked_add(value)
            .map(|_| ())
            .ok_or(field)
    }

    fn add(&mut self, flow: &Flow) {
        let protocol_offset = protocol_metric_offset(flow.protocol);
        self.metrics[FLOWS] += flow.flow_count;
        self.metrics[FLOWS + protocol_offset] += flow.flow_count;
        self.metrics[PACKETS] += flow.packets;
        self.metrics[PACKETS + protocol_offset] += flow.packets;
        self.metrics[BYTES] += flow.bytes;
        self.metrics[BYTES + protocol_offset] += flow.bytes;
        self.metrics[DURATION_SUM_MS] += flow.duration_sum_ms;
        self.metrics[DURATION_COUNT] += flow.flow_count;
        if let Some(sum) = flow.min_ttl_sum {
            self.metrics[MIN_TTL_SUM] += sum;
            self.metrics[MIN_TTL_COUNT] += flow.flow_count;
        }
        if let Some(sum) = flow.max_ttl_sum {
            self.metrics[MAX_TTL_SUM] += sum;
            self.metrics[MAX_TTL_COUNT] += flow.flow_count;
        }
        self.protocols[usize::from(flow.protocol) / 64] |=
            1_u64 << (usize::from(flow.protocol) % 64);
        self.source_addresses.insert(flow.source_address);
        self.destination_addresses.insert(flow.destination_address);
        insert_port(&mut self.source_ports, flow.source_port);
        insert_port(&mut self.destination_ports, flow.destination_port);
    }

    fn finish(
        self,
        scope: Scope,
    ) -> (
        ScopedTraffic,
        ScopedProtocols,
        [ScopedAddresses; 2],
        [ScopedPorts; 2],
    ) {
        let mut protocols = (0_u16..=255)
            .filter(|protocol| {
                let protocol = usize::from(*protocol);
                self.protocols[protocol / 64] & (1_u64 << (protocol % 64)) != 0
            })
            .map(|protocol| protocol.to_string())
            .collect::<Vec<_>>();
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
                    addresses: self.destination_addresses,
                },
                ScopedAddresses {
                    scope,
                    address_side: AddressSide::Source,
                    addresses: self.source_addresses,
                },
            ],
            [
                ScopedPorts {
                    scope,
                    port_side: AddressSide::Destination,
                    ports: self.destination_ports,
                },
                ScopedPorts {
                    scope,
                    port_side: AddressSide::Source,
                    ports: self.source_ports,
                },
            ],
        )
    }
}

pub(crate) fn reduce_to_bucket<R: Read>(
    mut input: R,
    key: BucketKey,
    selection: &FlowSelection,
) -> Result<CanonicalBucket, NfdumpError> {
    match reduce_stream(&mut input, selection) {
        Ok(scopes) => Ok(finish_bucket(scopes, key)),
        Err(error) => {
            drain_to_eof(&mut input);
            Err(error)
        }
    }
}

fn reduce_stream<R: Read>(
    input: &mut R,
    selection: &FlowSelection,
) -> Result<[ScopeAccumulator; 10], NfdumpError> {
    let mut header = [0_u8; 12];
    let read = read_fully(input, &mut header).map_err(|failure| {
        NfdumpError::new(
            Phase::Header,
            header_field_at(failure.bytes_read),
            ErrorReason::Io(failure.error),
        )
    })?;
    if read != header.len() {
        return Err(NfdumpError::new(
            Phase::Header,
            header_field_at(read),
            ErrorReason::ShortRead {
                expected: header.len(),
                actual: read,
            },
        ));
    }
    if &header[..8] != MAGIC {
        return Err(NfdumpError::new(
            Phase::Header,
            Field::Magic,
            ErrorReason::UnexpectedMagic,
        ));
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != CONTRACT_VERSION as u16 {
        return Err(NfdumpError::new(
            Phase::Header,
            Field::Version,
            ErrorReason::UnsupportedVersion(version),
        ));
    }
    let record_len = u16::from_le_bytes([header[10], header[11]]);
    if usize::from(record_len) != RECORD_LEN {
        return Err(NfdumpError::new(
            Phase::Header,
            Field::RecordLength,
            ErrorReason::UnexpectedRecordLength(record_len),
        ));
    }

    let mut scopes = std::array::from_fn(|_| ScopeAccumulator::default());
    let mut payload = [0_u8; MAX_BLOCK_BYTES];
    let mut block_index = 1_u64;
    let mut record_ordinal = 0_u64;
    loop {
        let mut count_bytes = [0_u8; 4];
        let read = read_fully(input, &mut count_bytes).map_err(|failure| {
            NfdumpError::new(
                Phase::BlockHeader,
                Field::BlockCount,
                ErrorReason::Io(failure.error),
            )
            .at_block(block_index)
        })?;
        if read == 0 {
            return Err(NfdumpError::new(
                Phase::BlockHeader,
                Field::BlockCount,
                ErrorReason::MissingTerminator,
            )
            .at_block(block_index));
        }
        if read != count_bytes.len() {
            return Err(NfdumpError::new(
                Phase::BlockHeader,
                Field::BlockCount,
                ErrorReason::ShortRead {
                    expected: count_bytes.len(),
                    actual: read,
                },
            )
            .at_block(block_index));
        }
        let count = u32::from_le_bytes(count_bytes);
        if count == 0 {
            ensure_eof(input, block_index)?;
            return Ok(scopes);
        }
        if count > MAX_BLOCK_RECORDS as u32 {
            return Err(NfdumpError::new(
                Phase::BlockHeader,
                Field::BlockCount,
                ErrorReason::ExcessiveBlockCount(count),
            )
            .at_block(block_index));
        }
        let payload_len = count as usize * RECORD_LEN;
        let read = read_fully(input, &mut payload[..payload_len]).map_err(|failure| {
            let failed_ordinal = record_ordinal + failure.bytes_read as u64 / RECORD_LEN as u64 + 1;
            NfdumpError::new(
                Phase::BlockPayload,
                record_field_at(failure.bytes_read % RECORD_LEN),
                ErrorReason::Io(failure.error),
            )
            .at_block(block_index)
            .at_record(failed_ordinal)
        })?;
        if read != payload_len {
            let failed_ordinal = record_ordinal + read as u64 / RECORD_LEN as u64 + 1;
            return Err(NfdumpError::new(
                Phase::BlockPayload,
                record_field_at(read % RECORD_LEN),
                ErrorReason::ShortRead {
                    expected: payload_len,
                    actual: read,
                },
            )
            .at_block(block_index)
            .at_record(failed_ordinal));
        }
        for record in payload[..payload_len].chunks_exact(RECORD_LEN) {
            record_ordinal += 1;
            let validated = validate_record(record, block_index, record_ordinal)?;
            if !matches_visibility(&validated, selection) {
                continue;
            }
            let flow = validated.into_flow();
            if selection.ip_prefix().is_some_and(|prefix| {
                !prefix.contains(&flow.source_address)
                    && !prefix.contains(&flow.destination_address)
            }) {
                continue;
            }
            let family_base = if flow.ip_version == IpVersion::V4 {
                0
            } else {
                5
            };
            let exact_index = family_base
                + exact_scope_index(flow.source_anonymized, flow.destination_anonymized);
            for index in [family_base, exact_index] {
                scopes[index].validate_add(&flow).map_err(|field| {
                    NfdumpError::new(Phase::Aggregate, field, ErrorReason::AggregateOverflow)
                        .at_block(block_index)
                        .at_record(record_ordinal)
                })?;
            }
            for index in [family_base, exact_index] {
                scopes[index].add(&flow);
            }
        }
        block_index += 1;
    }
}

struct ValidatedRecord<'a> {
    source_address: &'a [u8],
    destination_address: &'a [u8],
    ip_version: IpVersion,
    packets: i64,
    bytes: i64,
    flow_count: i64,
    duration_sum_ms: i64,
    source_port: u16,
    destination_port: u16,
    protocol: u8,
    source_anonymized: bool,
    destination_anonymized: bool,
    min_ttl_sum: Option<i64>,
    max_ttl_sum: Option<i64>,
}

impl ValidatedRecord<'_> {
    fn into_flow(self) -> Flow {
        let (source_address, destination_address) = match self.ip_version {
            IpVersion::V4 => (
                IpAddr::V4(Ipv4Addr::new(
                    self.source_address[0],
                    self.source_address[1],
                    self.source_address[2],
                    self.source_address[3],
                )),
                IpAddr::V4(Ipv4Addr::new(
                    self.destination_address[0],
                    self.destination_address[1],
                    self.destination_address[2],
                    self.destination_address[3],
                )),
            ),
            IpVersion::V6 => (
                IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(self.source_address)
                        .expect("validated source address has fixed width"),
                )),
                IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(self.destination_address)
                        .expect("validated destination address has fixed width"),
                )),
            ),
        };
        Flow {
            ip_version: self.ip_version,
            packets: self.packets,
            bytes: self.bytes,
            flow_count: self.flow_count,
            duration_sum_ms: self.duration_sum_ms,
            source_port: self.source_port,
            destination_port: self.destination_port,
            protocol: self.protocol,
            source_anonymized: self.source_anonymized,
            destination_anonymized: self.destination_anonymized,
            min_ttl_sum: self.min_ttl_sum,
            max_ttl_sum: self.max_ttl_sum,
            source_address,
            destination_address,
        }
    }
}

struct Flow {
    ip_version: IpVersion,
    packets: i64,
    bytes: i64,
    flow_count: i64,
    duration_sum_ms: i64,
    source_port: u16,
    destination_port: u16,
    protocol: u8,
    source_anonymized: bool,
    destination_anonymized: bool,
    min_ttl_sum: Option<i64>,
    max_ttl_sum: Option<i64>,
    source_address: IpAddr,
    destination_address: IpAddr,
}

fn validate_record(
    record: &[u8],
    block_index: u64,
    record_ordinal: u64,
) -> Result<ValidatedRecord<'_>, NfdumpError> {
    let error = |field, reason| {
        NfdumpError::new(Phase::Record, field, reason)
            .at_block(block_index)
            .at_record(record_ordinal)
    };
    let tag = record[69];
    if tag & !0b111 != 0 {
        return Err(error(Field::Tag, ErrorReason::InvalidTag(tag)));
    }
    let ip_version = if tag & 1 == 0 {
        IpVersion::V4
    } else {
        IpVersion::V6
    };
    if ip_version == IpVersion::V4 {
        if record[4..16].iter().any(|byte| *byte != 0) {
            return Err(error(Field::SourceAddress, ErrorReason::NonzeroIpv4Padding));
        }
        if record[20..32].iter().any(|byte| *byte != 0) {
            return Err(error(
                Field::DestinationAddress,
                ErrorReason::NonzeroIpv4Padding,
            ));
        }
    }
    let packets = signed_metric(record, 32, Field::Packets, &error)?;
    let bytes = signed_metric(record, 40, Field::Bytes, &error)?;
    let flow_count = signed_metric(record, 48, Field::FlowCount, &error)?;
    if flow_count == 0 {
        return Err(error(Field::FlowCount, ErrorReason::ZeroFlowCount));
    }
    let duration_ms = signed_metric(record, 56, Field::Duration, &error)?;
    let source_port = u16::from_le_bytes([record[64], record[65]]);
    let destination_port = u16::from_le_bytes([record[66], record[67]]);
    let protocol = record[68];
    if matches!(protocol, 1 | 58) && destination_port != 0 {
        return Err(error(
            Field::DestinationPort,
            ErrorReason::NonzeroIcmpDestinationPort(destination_port),
        ));
    }
    let min_ttl = record[70];
    let max_ttl = record[71];
    if min_ttl != 0 && max_ttl != 0 && min_ttl > max_ttl {
        return Err(error(
            Field::MinTtl,
            ErrorReason::InvalidTtlOrder {
                minimum: min_ttl,
                maximum: max_ttl,
            },
        ));
    }
    let duration_sum_ms = duration_ms
        .checked_mul(flow_count)
        .ok_or_else(|| error(Field::Duration, ErrorReason::AggregateOverflow))?;
    let min_ttl_sum = if min_ttl == 0 {
        None
    } else {
        Some(
            i64::from(min_ttl)
                .checked_mul(flow_count)
                .ok_or_else(|| error(Field::MinTtl, ErrorReason::AggregateOverflow))?,
        )
    };
    let max_ttl_sum = if max_ttl == 0 {
        None
    } else {
        Some(
            i64::from(max_ttl)
                .checked_mul(flow_count)
                .ok_or_else(|| error(Field::MaxTtl, ErrorReason::AggregateOverflow))?,
        )
    };
    Ok(ValidatedRecord {
        source_address: &record[..16],
        destination_address: &record[16..32],
        ip_version,
        packets,
        bytes,
        flow_count,
        duration_sum_ms,
        source_port,
        destination_port,
        protocol,
        source_anonymized: tag & 0b010 != 0,
        destination_anonymized: tag & 0b100 != 0,
        min_ttl_sum,
        max_ttl_sum,
    })
}

fn signed_metric<F>(
    record: &[u8],
    offset: usize,
    field: Field,
    error: &F,
) -> Result<i64, NfdumpError>
where
    F: Fn(Field, ErrorReason) -> NfdumpError,
{
    let value = u64::from_le_bytes(
        record[offset..offset + 8]
            .try_into()
            .expect("metric has fixed width"),
    );
    i64::try_from(value).map_err(|_| error(field, ErrorReason::NumericOverflow(value)))
}

fn matches_visibility(record: &ValidatedRecord<'_>, selection: &FlowSelection) -> bool {
    selection.src_visibility().is_none_or(|required| {
        matches!(required, ExactVisibility::Anonymized) == record.source_anonymized
    }) && selection.dst_visibility().is_none_or(|required| {
        matches!(required, ExactVisibility::Anonymized) == record.destination_anonymized
    })
}

const fn exact_scope_index(source_anonymized: bool, destination_anonymized: bool) -> usize {
    match (source_anonymized, destination_anonymized) {
        (true, true) => 1,
        (true, false) => 2,
        (false, true) => 3,
        (false, false) => 4,
    }
}

const fn protocol_metric_offset(protocol: u8) -> usize {
    match protocol {
        6 => 1,
        17 => 2,
        1 | 58 => 3,
        _ => 4,
    }
}

fn insert_port(ports: &mut FixedBitSet, port: u16) {
    if ports.len() < 65_536 {
        ports.grow(65_536);
    }
    ports.insert(usize::from(port));
}

fn ensure_eof<R: Read>(input: &mut R, block_index: u64) -> Result<(), NfdumpError> {
    let mut byte = [0_u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                return Err(NfdumpError::new(
                    Phase::End,
                    Field::StreamEnd,
                    ErrorReason::TrailingBytes,
                )
                .at_block(block_index));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(
                    NfdumpError::new(Phase::End, Field::StreamEnd, ErrorReason::Io(error))
                        .at_block(block_index),
                );
            }
        }
    }
}

struct ReadFailure {
    bytes_read: usize,
    error: io::Error,
}

fn read_fully<R: Read>(input: &mut R, mut bytes: &mut [u8]) -> Result<usize, ReadFailure> {
    let expected = bytes.len();
    while !bytes.is_empty() {
        match input.read(bytes) {
            Ok(0) => break,
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(ReadFailure {
                    bytes_read: expected - bytes.len(),
                    error,
                });
            }
        }
    }
    Ok(expected - bytes.len())
}

fn drain_to_eof<R: Read>(input: &mut R) {
    let mut buffer = [0_u8; 8_192];
    loop {
        match input.read(&mut buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

const fn header_field_at(offset: usize) -> Field {
    match offset {
        0..=7 => Field::Magic,
        8..=9 => Field::Version,
        _ => Field::RecordLength,
    }
}

const fn record_field_at(offset: usize) -> Field {
    match offset {
        0..=15 => Field::SourceAddress,
        16..=31 => Field::DestinationAddress,
        32..=39 => Field::Packets,
        40..=47 => Field::Bytes,
        48..=55 => Field::FlowCount,
        56..=63 => Field::Duration,
        64..=65 => Field::SourcePort,
        66..=67 => Field::DestinationPort,
        68 => Field::Protocol,
        69 => Field::Tag,
        70 => Field::MinTtl,
        _ => Field::MaxTtl,
    }
}

fn finish_bucket(scopes: [ScopeAccumulator; 10], key: BucketKey) -> CanonicalBucket {
    const VISIBILITIES: [(Visibility, Visibility); 5] = [
        (Visibility::All, Visibility::All),
        (Visibility::Anonymized, Visibility::Anonymized),
        (Visibility::Anonymized, Visibility::Literal),
        (Visibility::Literal, Visibility::Anonymized),
        (Visibility::Literal, Visibility::Literal),
    ];
    let mut traffic = Vec::with_capacity(10);
    let mut protocols = Vec::with_capacity(10);
    let mut addresses = Vec::with_capacity(20);
    let mut ports = Vec::with_capacity(20);
    for (index, accumulator) in scopes.into_iter().enumerate() {
        let scope = Scope::new(
            if index < 5 {
                IpVersion::V4
            } else {
                IpVersion::V6
            },
            VISIBILITIES[index % 5].0,
            VISIBILITIES[index % 5].1,
        );
        let (scope_traffic, scope_protocols, scope_addresses, scope_ports) =
            accumulator.finish(scope);
        traffic.push(scope_traffic);
        protocols.push(scope_protocols);
        addresses.extend(scope_addresses);
        ports.extend(scope_ports);
    }
    let mut five_minute_starts = BTreeSet::new();
    if key.granularity == Granularity::FiveMinutes {
        five_minute_starts.insert(key.bucket_start);
    }
    CanonicalBucket {
        key,
        coverage: BucketCoverage::complete_unit(),
        traffic,
        protocols,
        addresses,
        ports,
        five_minute_starts,
    }
}

fn traffic_metrics(metrics: [i64; METRIC_COUNT]) -> TrafficMetrics {
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
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;
    use crate::domain::{Granularity, Scope};

    fn key() -> BucketKey {
        BucketKey::new(
            "edge-a",
            Granularity::FiveMinutes,
            1_700_000_000,
            1_700_000_300,
        )
    }

    fn base_record() -> [u8; RECORD_LEN] {
        ONE_V4_TEST_STREAM[16..88]
            .try_into()
            .expect("the exported fixture contains one fixed record")
    }

    fn stream(records: &[[u8; RECORD_LEN]]) -> Vec<u8> {
        let mut bytes = vec![65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0];
        bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for record in records {
            bytes.extend_from_slice(record);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    #[test]
    fn empty_stream_returns_ten_dense_scopes() {
        let bytes = [
            65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, // Header.
            0, 0, 0, 0, // Mandatory end marker.
        ];

        let bucket =
            reduce_to_bucket(Cursor::new(bytes), key(), &FlowSelection::default()).unwrap();

        assert_eq!(bucket.traffic.len(), 10);
        assert_eq!(bucket.protocols.len(), 10);
        assert_eq!(bucket.addresses.len(), 20);
        assert_eq!(bucket.ports.len(), 20);
        assert_eq!(bucket.five_minute_starts, [1_700_000_000].into());
    }

    #[test]
    fn one_ipv4_record_reduces_into_all_and_exact_scopes() {
        let bucket = reduce_to_bucket(
            Cursor::new(ONE_V4_TEST_STREAM),
            key(),
            &FlowSelection::default(),
        )
        .unwrap();

        for index in [0, 4] {
            let metrics = &bucket.traffic[index].metrics;
            assert_eq!(metrics.flows, 1);
            assert_eq!(metrics.flows_tcp, 1);
            assert_eq!(metrics.packets, 2);
            assert_eq!(metrics.bytes, 128);
            assert_eq!(metrics.duration_sum_ms, 1_000);
            assert_eq!(metrics.duration_count, 1);
            assert_eq!(metrics.min_ttl_sum, 31);
            assert_eq!(metrics.min_ttl_count, 1);
            assert_eq!(metrics.max_ttl_sum, 64);
            assert_eq!(metrics.max_ttl_count, 1);
            assert_eq!(bucket.protocols[index].protocols, ["6"]);
        }
        assert_eq!(
            bucket.addresses[0].addresses,
            [IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2))]
                .into_iter()
                .collect::<AddressSet>()
        );
        assert_eq!(
            bucket.addresses[1].addresses,
            [IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))]
                .into_iter()
                .collect::<AddressSet>()
        );
        assert!(bucket.ports[0].ports.contains(55_000));
        assert!(bucket.ports[1].ports.contains(443));
    }

    #[test]
    fn icmp_requires_and_retains_a_zero_destination_port() {
        let mut invalid = ONE_V4_TEST_STREAM.to_vec();
        invalid[16 + 66..16 + 68].copy_from_slice(&[1, 0]);
        invalid[16 + 68] = 1;

        let error =
            reduce_to_bucket(Cursor::new(invalid), key(), &FlowSelection::default()).unwrap_err();

        assert_eq!(error.record_ordinal, Some(1));
        assert_eq!(error.field, Field::DestinationPort);

        let mut valid = ONE_V4_TEST_STREAM.to_vec();
        valid[16 + 66..16 + 68].copy_from_slice(&[0, 0]);
        valid[16 + 68] = 1;
        let bucket =
            reduce_to_bucket(Cursor::new(valid), key(), &FlowSelection::default()).unwrap();
        assert!(bucket.ports[0].ports.contains(0));
    }

    #[test]
    fn ipv6_visibility_prefix_and_weighted_missing_ttl_reduce_correctly() {
        let mut record = base_record();
        record[..16].copy_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        record[16..32]
            .copy_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        record[48..56].copy_from_slice(&[3, 0, 0, 0, 0, 0, 0, 0]);
        record[56..64].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
        record[64..68].copy_from_slice(&[0, 0, 0, 0]);
        record[68] = 58;
        record[69] = 0b011;
        record[70] = 0;
        record[71] = 5;
        let selection = FlowSelection::from_payload(Some(&serde_json::json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": "2001:db8::/32",
            "src_visibility": "anonymized",
            "dst_visibility": "literal",
        })))
        .unwrap();

        let bucket = reduce_to_bucket(Cursor::new(stream(&[record])), key(), &selection).unwrap();

        for index in [5, 7] {
            let metrics = &bucket.traffic[index].metrics;
            assert_eq!(metrics.flows, 3);
            assert_eq!(metrics.flows_icmp, 3);
            assert_eq!(metrics.duration_sum_ms, 6);
            assert_eq!(metrics.duration_count, 3);
            assert_eq!(metrics.min_ttl_count, 0);
            assert_eq!(metrics.max_ttl_sum, 15);
            assert_eq!(metrics.max_ttl_count, 3);
            assert_eq!(bucket.protocols[index].protocols, ["58"]);
        }
        assert_eq!(
            bucket.addresses[10].addresses,
            [IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 2))]
                .into_iter()
                .collect::<AddressSet>()
        );
        assert!(bucket.ports[10].ports.contains(0));
    }

    #[test]
    fn synthetic_tunnel_and_outer_record_keep_independent_weights_and_ttl() {
        let mut tunnel = base_record();
        tunnel[32..40].copy_from_slice(&[10, 0, 0, 0, 0, 0, 0, 0]);
        tunnel[40..48].copy_from_slice(&[100, 0, 0, 0, 0, 0, 0, 0]);
        tunnel[48..56].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        tunnel[56..64].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
        tunnel[68] = 47;
        tunnel[70] = 0;
        tunnel[71] = 0;
        let mut outer = tunnel;
        outer[48..56].copy_from_slice(&[3, 0, 0, 0, 0, 0, 0, 0]);
        outer[68] = 6;
        outer[70] = 4;
        outer[71] = 5;

        let bucket = reduce_to_bucket(
            Cursor::new(stream(&[tunnel, outer])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap();
        let metrics = &bucket.traffic[0].metrics;

        assert_eq!(metrics.flows, 4);
        assert_eq!(metrics.packets, 20);
        assert_eq!(metrics.bytes, 200);
        assert_eq!(metrics.duration_sum_ms, 8);
        assert_eq!(metrics.duration_count, 4);
        assert_eq!(metrics.min_ttl_sum, 12);
        assert_eq!(metrics.min_ttl_count, 3);
        assert_eq!(metrics.max_ttl_sum, 15);
        assert_eq!(metrics.max_ttl_count, 3);
        assert_eq!(bucket.protocols[0].protocols, ["47", "6"]);
    }

    #[test]
    fn malformed_excluded_record_is_rejected_and_input_is_drained() {
        let mut record = base_record();
        record[32..40].copy_from_slice(&[255; 8]);
        let bytes = stream(&[record]);
        let length = bytes.len() as u64;
        let selection = FlowSelection::from_payload(Some(&serde_json::json!({
            "version": 1,
            "kind": "flows",
            "src_visibility": "anonymized",
        })))
        .unwrap();
        let mut input = Cursor::new(bytes);

        let error = reduce_to_bucket(&mut input, key(), &selection).unwrap_err();

        assert_eq!(error.phase, Phase::Record);
        assert_eq!(error.block_index, Some(1));
        assert_eq!(error.record_ordinal, Some(1));
        assert_eq!(error.field, Field::Packets);
        assert_eq!(input.position(), length);
    }

    #[test]
    fn rejects_tag_padding_zero_flows_and_ttl_order_with_field_diagnostics() {
        let cases = [
            (69, 8, Field::Tag, "reserved bits"),
            (4, 1, Field::SourceAddress, "nonzero trailing padding"),
            (20, 1, Field::DestinationAddress, "nonzero trailing padding"),
            (48, 0, Field::FlowCount, "must be positive"),
            (70, 65, Field::MinTtl, "exceeds maximum TTL"),
        ];
        for (offset, value, expected_field, expected_reason) in cases {
            let mut record = base_record();
            record[offset] = value;

            let error = reduce_to_bucket(
                Cursor::new(stream(&[record])),
                key(),
                &FlowSelection::default(),
            )
            .unwrap_err();

            assert_eq!(error.record_ordinal, Some(1));
            assert_eq!(error.field, expected_field);
            assert!(error.to_string().contains(expected_reason), "{error}");
        }

        let mut independently_missing_max = base_record();
        independently_missing_max[70] = 64;
        independently_missing_max[71] = 0;
        let bucket = reduce_to_bucket(
            Cursor::new(stream(&[independently_missing_max])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap();
        assert_eq!(bucket.traffic[0].metrics.min_ttl_sum, 64);
        assert_eq!(bucket.traffic[0].metrics.max_ttl_count, 0);
    }

    #[test]
    fn rejects_numeric_weighted_and_running_aggregate_overflow() {
        let mut numeric = base_record();
        numeric[32..40].copy_from_slice(&[255; 8]);
        let error = reduce_to_bucket(
            Cursor::new(stream(&[numeric])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap_err();
        assert_eq!(error.phase, Phase::Record);
        assert_eq!(error.field, Field::Packets);
        assert!(error.to_string().contains("value 18446744073709551615"));

        let mut weighted = base_record();
        weighted[48..56].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0]);
        weighted[56..64].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 127]);
        let error = reduce_to_bucket(
            Cursor::new(stream(&[weighted])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap_err();
        assert_eq!(error.phase, Phase::Record);
        assert_eq!(error.field, Field::Duration);
        assert!(error.to_string().contains("aggregate range"));

        let mut maximum = base_record();
        maximum[32..40].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 127]);
        maximum[56..64].copy_from_slice(&[0; 8]);
        maximum[70] = 0;
        maximum[71] = 0;
        let mut one = maximum;
        one[32..40].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        let error = reduce_to_bucket(
            Cursor::new(stream(&[maximum, one])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap_err();
        assert_eq!(error.phase, Phase::Aggregate);
        assert_eq!(error.record_ordinal, Some(2));
        assert_eq!(error.field, Field::Packets);
    }

    #[test]
    fn header_blocks_terminator_and_physical_eof_are_exact() {
        let empty = [65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 0, 0, 0, 0];
        for (offset, value, field) in [
            (0, 0, Field::Magic),
            (8, 2, Field::Version),
            (10, 71, Field::RecordLength),
        ] {
            let mut bytes = empty;
            bytes[offset] = value;
            let error =
                reduce_to_bucket(Cursor::new(bytes), key(), &FlowSelection::default()).unwrap_err();
            assert_eq!(error.phase, Phase::Header);
            assert_eq!(error.field, field);
        }

        let excessive = [65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 1, 8, 0, 0];
        let error =
            reduce_to_bucket(Cursor::new(excessive), key(), &FlowSelection::default()).unwrap_err();
        assert_eq!(error.field, Field::BlockCount);
        assert!(error.to_string().contains("2049 exceeds 2048"));

        let mut missing_end = ONE_V4_TEST_STREAM[..88].to_vec();
        let error = reduce_to_bucket(
            Cursor::new(&mut missing_end),
            key(),
            &FlowSelection::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing mandatory end marker"));

        let mut trailing = empty.to_vec();
        trailing.extend_from_slice(&[99, 100]);
        let length = trailing.len() as u64;
        let mut input = Cursor::new(trailing);
        let error = reduce_to_bucket(&mut input, key(), &FlowSelection::default()).unwrap_err();
        assert_eq!(error.phase, Phase::End);
        assert_eq!(error.field, Field::StreamEnd);
        assert_eq!(input.position(), length);
    }

    #[test]
    fn every_truncation_and_partial_record_reports_its_location() {
        for end in 0..ONE_V4_TEST_STREAM.len() {
            let error = reduce_to_bucket(
                Cursor::new(&ONE_V4_TEST_STREAM[..end]),
                key(),
                &FlowSelection::default(),
            )
            .unwrap_err();
            assert!(!error.to_string().is_empty());
        }

        let record = base_record();
        let mut partial_second = vec![65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, 2, 0, 0, 0];
        partial_second.extend_from_slice(&record);
        partial_second.extend_from_slice(&record[..41]);
        let error = reduce_to_bucket(
            Cursor::new(partial_second),
            key(),
            &FlowSelection::default(),
        )
        .unwrap_err();
        assert_eq!(error.phase, Phase::BlockPayload);
        assert_eq!(error.block_index, Some(1));
        assert_eq!(error.record_ordinal, Some(2));
        assert_eq!(error.field, Field::Bytes);
    }

    #[test]
    fn payload_io_error_keeps_partial_record_location_and_then_drains_to_eof() {
        struct ErrorOnceReader {
            bytes: Vec<u8>,
            position: usize,
            fail_at: usize,
            failed: bool,
        }

        impl Read for ErrorOnceReader {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if !self.failed && self.position == self.fail_at {
                    self.failed = true;
                    return Err(io::Error::other("injected read failure"));
                }
                if self.position == self.bytes.len() {
                    return Ok(0);
                }
                let available_before_failure = if self.failed {
                    self.bytes.len() - self.position
                } else {
                    self.fail_at - self.position
                };
                let read = output
                    .len()
                    .min(available_before_failure)
                    .min(self.bytes.len() - self.position);
                output[..read].copy_from_slice(&self.bytes[self.position..self.position + read]);
                self.position += read;
                Ok(read)
            }
        }

        let record = base_record();
        let bytes = stream(&[record, record]);
        let length = bytes.len();
        let mut input = ErrorOnceReader {
            bytes,
            position: 0,
            fail_at: 16 + RECORD_LEN + 41,
            failed: false,
        };

        let error = reduce_to_bucket(&mut input, key(), &FlowSelection::default()).unwrap_err();

        assert_eq!(error.phase, Phase::BlockPayload);
        assert_eq!(error.block_index, Some(1));
        assert_eq!(error.record_ordinal, Some(2));
        assert_eq!(error.field, Field::Bytes);
        assert!(error.to_string().contains("injected read failure"));
        assert_eq!(input.position, length);
    }

    #[test]
    fn diagnostics_count_blocks_and_records_globally() {
        let record = base_record();
        let mut bad_record = record;
        bad_record[69] = 8;
        let mut bytes = vec![
            65, 84, 76, 78, 70, 76, 79, 87, 1, 0, 72, 0, // Header.
            1, 0, 0, 0, // Block one.
        ];
        bytes.extend_from_slice(&record);
        bytes.extend_from_slice(&[1, 0, 0, 0]);
        bytes.extend_from_slice(&bad_record);
        bytes.extend_from_slice(&[0, 0, 0, 0]);

        let error =
            reduce_to_bucket(Cursor::new(bytes), key(), &FlowSelection::default()).unwrap_err();

        assert_eq!(error.block_index, Some(2));
        assert_eq!(error.record_ordinal, Some(2));
        assert_eq!(error.field, Field::Tag);
    }

    #[test]
    fn canonical_output_orders_scopes_and_textual_protocols() {
        let mut udp = base_record();
        udp[0..4].copy_from_slice(&[192, 0, 2, 2]);
        udp[16..20].copy_from_slice(&[198, 51, 100, 1]);
        udp[68] = 17;
        udp[69] = 0b100;
        let mut tcp = base_record();
        tcp[0..4].copy_from_slice(&[192, 0, 2, 1]);
        tcp[16..20].copy_from_slice(&[198, 51, 100, 2]);
        tcp[69] = 0b100;

        let bucket = reduce_to_bucket(
            Cursor::new(stream(&[udp, tcp])),
            key(),
            &FlowSelection::default(),
        )
        .unwrap();

        assert_eq!(
            bucket.traffic[3].scope,
            Scope::new(IpVersion::V4, Visibility::Literal, Visibility::Anonymized)
        );
        assert_eq!(bucket.protocols[0].protocols, ["17", "6"]);
        assert_eq!(
            bucket.addresses[1].addresses,
            [
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
                IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)),
            ]
            .into_iter()
            .collect::<AddressSet>()
        );
    }
}
