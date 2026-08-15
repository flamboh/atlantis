//! Canonical, adapter-independent NetFlow observations and statistical buckets.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use fixedbitset::FixedBitSet;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

const PORT_COUNT: usize = 65_536;

/// Largest integer accepted by SQLite's INTEGER storage class.
pub const MAX_SQLITE_INTEGER: i64 = i64::MAX;

/// Failures rejected at the canonical domain boundary.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("source and destination addresses must use the same IP version")]
    MixedIpVersions,
    #[error("{0} must be a non-negative signed 64-bit integer")]
    NegativeMetric(&'static str),
    #[error("{0} exceeds SQLite signed 64-bit integer range")]
    MetricOverflow(&'static str),
    #[error("selection must be an object")]
    SelectionMustBeObject,
    #[error("Unknown selection keys: {0}")]
    UnknownSelectionKeys(String),
    #[error("selection version must be 1")]
    InvalidSelectionVersion,
    #[error("selection kind must be 'all' or 'flows'")]
    InvalidSelectionKind,
    #[error("selection kind 'all' cannot define flow criteria")]
    AllSelectionHasCriteria,
    #[error("Invalid selection ip_prefix: {0}")]
    InvalidIpPrefix(String),
    #[error("selection {0} must be 'literal' or 'anonymized'")]
    InvalidVisibility(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IpVersion {
    V4,
    V6,
}

impl IpVersion {
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }

    #[must_use]
    pub const fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => Self::V4,
            IpAddr::V6(_) => Self::V6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Granularity {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "30m")]
    ThirtyMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "1d")]
    OneDay,
}

impl Granularity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::ThirtyMinutes => "30m",
            Self::OneHour => "1h",
            Self::OneDay => "1d",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    All,
    Anonymized,
    Literal,
}

impl Visibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Anonymized => "anonymized",
            Self::Literal => "literal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExactVisibility {
    Anonymized,
    Literal,
}

impl ExactVisibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anonymized => "anonymized",
            Self::Literal => "literal",
        }
    }
}

impl From<ExactVisibility> for Visibility {
    fn from(value: ExactVisibility) -> Self {
        match value {
            ExactVisibility::Anonymized => Self::Anonymized,
            ExactVisibility::Literal => Self::Literal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddressSide {
    Destination,
    Source,
}

impl AddressSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Destination => "destination",
            Self::Source => "source",
        }
    }
}

pub type PortSide = AddressSide;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortRange {
    Low,
    High,
}

impl PortRange {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

/// An adapter-normalized flow. Optional measurements preserve missingness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowObservation {
    pub src_ip: IpAddr,
    pub dst_ip: IpAddr,
    pub protocol: u8,
    pub packets: i64,
    pub bytes: i64,
    pub src_tos: u8,
    pub time_received_ms: Option<i64>,
    pub time_end_ms: Option<i64>,
    pub time_start_ms: Option<i64>,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub dst_tos: u8,
    pub duration_ms: Option<i64>,
    pub min_ttl: Option<u8>,
    pub max_ttl: Option<u8>,
    pub flow_count: i64,
}

impl FlowObservation {
    pub fn new(
        src_ip: IpAddr,
        dst_ip: IpAddr,
        protocol: u8,
        packets: i64,
        bytes: i64,
        src_tos: u8,
    ) -> Result<Self, DomainError> {
        if IpVersion::of(src_ip) != IpVersion::of(dst_ip) {
            return Err(DomainError::MixedIpVersions);
        }
        ensure_non_negative(packets, "packets")?;
        ensure_non_negative(bytes, "bytes")?;
        Ok(Self {
            src_ip,
            dst_ip,
            protocol,
            packets,
            bytes,
            src_tos,
            time_received_ms: None,
            time_end_ms: None,
            time_start_ms: None,
            src_port: None,
            dst_port: None,
            dst_tos: 0,
            duration_ms: None,
            min_ttl: None,
            max_ttl: None,
            flow_count: 1,
        })
    }

    #[must_use]
    pub const fn ip_version(&self) -> IpVersion {
        IpVersion::of(self.src_ip)
    }

    #[must_use]
    pub const fn with_ports(mut self, src_port: Option<u16>, dst_port: Option<u16>) -> Self {
        self.src_port = src_port;
        self.dst_port = dst_port;
        self
    }

    pub fn with_measurements(
        mut self,
        duration_ms: Option<i64>,
        min_ttl: Option<u8>,
        max_ttl: Option<u8>,
    ) -> Result<Self, DomainError> {
        if let Some(duration) = duration_ms {
            ensure_non_negative(duration, "duration_ms")?;
        }
        self.duration_ms = duration_ms;
        self.min_ttl = min_ttl;
        self.max_ttl = max_ttl;
        Ok(self)
    }

    pub fn with_flow_count(mut self, flow_count: i64) -> Result<Self, DomainError> {
        ensure_non_negative(flow_count, "flow_count")?;
        self.flow_count = flow_count;
        Ok(self)
    }
}

/// A validated predicate shared by all input adapters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlowSelection {
    ip_prefix: Option<IpNet>,
    src_visibility: Option<ExactVisibility>,
    dst_visibility: Option<ExactVisibility>,
}

impl FlowSelection {
    pub fn from_payload(payload: Option<&Value>) -> Result<Self, DomainError> {
        let Some(payload) = payload else {
            return Ok(Self::default());
        };
        let object = payload
            .as_object()
            .ok_or(DomainError::SelectionMustBeObject)?;
        let unknown = object
            .keys()
            .filter(|key| {
                !matches!(
                    key.as_str(),
                    "version" | "kind" | "ip_prefix" | "src_visibility" | "dst_visibility"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(DomainError::UnknownSelectionKeys(format!("{unknown:?}")));
        }
        if object.get("version").is_some_and(|version| version != 1) {
            return Err(DomainError::InvalidSelectionVersion);
        }
        let kind = match object.get("kind") {
            None | Some(Value::Null) => None,
            Some(Value::String(kind)) => Some(kind.as_str()),
            Some(_) => return Err(DomainError::InvalidSelectionKind),
        };
        if !matches!(kind, None | Some("all" | "flows")) {
            return Err(DomainError::InvalidSelectionKind);
        }
        if kind == Some("all")
            && ["ip_prefix", "src_visibility", "dst_visibility"]
                .iter()
                .any(|key| !is_empty_value(object.get(*key)))
        {
            return Err(DomainError::AllSelectionHasCriteria);
        }

        let raw_prefix = optional_non_empty_string(object, "ip_prefix")?;
        let ip_prefix = raw_prefix
            .map(|prefix| {
                prefix
                    .parse::<IpNet>()
                    .map_err(|_| DomainError::InvalidIpPrefix(format!("{prefix:?}")))
                    .map(|network| network.trunc())
            })
            .transpose()?;
        Ok(Self {
            ip_prefix,
            src_visibility: parse_visibility(object, "src_visibility")?,
            dst_visibility: parse_visibility(object, "dst_visibility")?,
        })
    }

    #[must_use]
    pub const fn is_unrestricted(&self) -> bool {
        self.ip_prefix.is_none() && self.src_visibility.is_none() && self.dst_visibility.is_none()
    }

    #[must_use]
    pub const fn src_visibility(&self) -> Option<ExactVisibility> {
        self.src_visibility
    }

    #[must_use]
    pub const fn dst_visibility(&self) -> Option<ExactVisibility> {
        self.dst_visibility
    }

    #[must_use]
    pub const fn ip_prefix(&self) -> Option<&IpNet> {
        self.ip_prefix.as_ref()
    }

    #[must_use]
    pub fn matches(&self, observation: &FlowObservation) -> bool {
        let prefix_matches = self.ip_prefix.as_ref().is_none_or(|prefix| {
            prefix.contains(&observation.src_ip) || prefix.contains(&observation.dst_ip)
        });
        prefix_matches && self.allows_src_tos(observation.src_tos)
    }

    #[must_use]
    pub fn allows_src_tos(&self, src_tos: u8) -> bool {
        let (source, destination) = exact_visibility_pair_from_tos(src_tos);
        self.src_visibility
            .is_none_or(|required| required == source)
            && self
                .dst_visibility
                .is_none_or(|required| required == destination)
    }

    #[must_use]
    pub fn nfdump_prefix_filter(&self) -> Option<String> {
        self.ip_prefix
            .as_ref()
            .map(|prefix| format!("net {prefix}"))
    }

    #[must_use]
    pub fn normalized_payload(&self) -> Value {
        if self.is_unrestricted() {
            return json!({"version": 1, "kind": "all"});
        }
        json!({
            "version": 1,
            "kind": "flows",
            "ip_prefix": self.ip_prefix.map(|prefix| prefix.to_string()),
            "src_visibility": self.src_visibility.map(ExactVisibility::as_str),
            "dst_visibility": self.dst_visibility.map(ExactVisibility::as_str),
        })
    }
}

fn optional_non_empty_string<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
) -> Result<Option<&'a str>, DomainError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) if key == "kind" => Err(DomainError::InvalidSelectionKind),
        Some(value) if key == "ip_prefix" => {
            Err(DomainError::InvalidIpPrefix(format!("{value:?}")))
        }
        Some(_) => Err(DomainError::InvalidVisibility(key)),
    }
}

fn is_empty_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(_) => false,
    }
}

fn parse_visibility(
    object: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<ExactVisibility>, DomainError> {
    match optional_non_empty_string(object, key)? {
        None => Ok(None),
        Some("literal") => Ok(Some(ExactVisibility::Literal)),
        Some("anonymized") => Ok(Some(ExactVisibility::Anonymized)),
        Some(_) => Err(DomainError::InvalidVisibility(key)),
    }
}

fn exact_visibility_pair_from_tos(src_tos: u8) -> (ExactVisibility, ExactVisibility) {
    match src_tos & 3 {
        0 => (ExactVisibility::Literal, ExactVisibility::Literal),
        1 => (ExactVisibility::Literal, ExactVisibility::Anonymized),
        2 => (ExactVisibility::Anonymized, ExactVisibility::Literal),
        3 => (ExactVisibility::Anonymized, ExactVisibility::Anonymized),
        _ => unreachable!("two masked bits have only four values"),
    }
}

/// Interpret only the two low source-ToS bits as endpoint visibility flags.
#[must_use]
pub fn visibility_pair_from_tos(src_tos: u8) -> (Visibility, Visibility) {
    let (source, destination) = exact_visibility_pair_from_tos(src_tos);
    (source.into(), destination.into())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BucketKey {
    pub source_id: String,
    pub granularity: Granularity,
    pub bucket_start: i64,
    pub bucket_end: i64,
}

impl BucketKey {
    #[must_use]
    pub fn new(
        source_id: impl Into<String>,
        granularity: Granularity,
        bucket_start: i64,
        bucket_end: i64,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            granularity,
            bucket_start,
            bucket_end,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    pub ip_version: IpVersion,
    pub src_visibility: Visibility,
    pub dst_visibility: Visibility,
}

impl Scope {
    #[must_use]
    pub const fn new(
        ip_version: IpVersion,
        src_visibility: Visibility,
        dst_visibility: Visibility,
    ) -> Self {
        Self {
            ip_version,
            src_visibility,
            dst_visibility,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupedTrafficFact {
    pub ip_version: IpVersion,
    pub protocol: u8,
    pub src_tos: u8,
    pub flows: i64,
    pub packets: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedAddressesFact {
    pub scope: Scope,
    pub address_side: AddressSide,
    pub addresses: BTreeSet<IpAddr>,
}

impl ScopedAddressesFact {
    #[must_use]
    pub fn new(
        scope: Scope,
        address_side: AddressSide,
        addresses: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        Self {
            scope,
            address_side,
            addresses: addresses.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatisticalFact {
    Observation(FlowObservation),
    GroupedTraffic(GroupedTrafficFact),
    ScopedAddresses(ScopedAddressesFact),
}

impl From<FlowObservation> for StatisticalFact {
    fn from(value: FlowObservation) -> Self {
        Self::Observation(value)
    }
}

impl From<GroupedTrafficFact> for StatisticalFact {
    fn from(value: GroupedTrafficFact) -> Self {
        Self::GroupedTraffic(value)
    }
}

impl From<ScopedAddressesFact> for StatisticalFact {
    fn from(value: ScopedAddressesFact) -> Self {
        Self::ScopedAddresses(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrafficMetrics {
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
    pub min_ttl_sum: i64,
    pub min_ttl_count: i64,
    pub max_ttl_sum: i64,
    pub max_ttl_count: i64,
}

impl TrafficMetrics {
    fn add_traffic(
        &mut self,
        protocol: u8,
        flows: i64,
        packets: i64,
        bytes: i64,
    ) -> Result<(), DomainError> {
        ensure_non_negative(flows, "flows")?;
        ensure_non_negative(packets, "packets")?;
        ensure_non_negative(bytes, "bytes")?;
        add_metric(&mut self.flows, flows, "flows")?;
        add_metric(&mut self.packets, packets, "packets")?;
        add_metric(&mut self.bytes, bytes, "bytes")?;
        let (protocol_flows, protocol_packets, protocol_bytes, names) = match protocol {
            6 => (
                &mut self.flows_tcp,
                &mut self.packets_tcp,
                &mut self.bytes_tcp,
                ("flows_tcp", "packets_tcp", "bytes_tcp"),
            ),
            17 => (
                &mut self.flows_udp,
                &mut self.packets_udp,
                &mut self.bytes_udp,
                ("flows_udp", "packets_udp", "bytes_udp"),
            ),
            1 | 58 => (
                &mut self.flows_icmp,
                &mut self.packets_icmp,
                &mut self.bytes_icmp,
                ("flows_icmp", "packets_icmp", "bytes_icmp"),
            ),
            _ => (
                &mut self.flows_other,
                &mut self.packets_other,
                &mut self.bytes_other,
                ("flows_other", "packets_other", "bytes_other"),
            ),
        };
        add_metric(protocol_flows, flows, names.0)?;
        add_metric(protocol_packets, packets, names.1)?;
        add_metric(protocol_bytes, bytes, names.2)
    }

    fn add_observation(&mut self, observation: &FlowObservation) -> Result<(), DomainError> {
        self.add_traffic(
            observation.protocol,
            observation.flow_count,
            observation.packets,
            observation.bytes,
        )?;
        if let Some(duration) = observation.duration_ms {
            let weighted = multiply_metric(duration, observation.flow_count, "duration_sum_ms")?;
            add_metric(&mut self.duration_sum_ms, weighted, "duration_sum_ms")?;
            add_metric(
                &mut self.duration_count,
                observation.flow_count,
                "duration_count",
            )?;
        }
        if let Some(min_ttl) = observation.min_ttl {
            let weighted =
                multiply_metric(i64::from(min_ttl), observation.flow_count, "min_ttl_sum")?;
            add_metric(&mut self.min_ttl_sum, weighted, "min_ttl_sum")?;
            add_metric(
                &mut self.min_ttl_count,
                observation.flow_count,
                "min_ttl_count",
            )?;
        }
        if let Some(max_ttl) = observation.max_ttl {
            let weighted =
                multiply_metric(i64::from(max_ttl), observation.flow_count, "max_ttl_sum")?;
            add_metric(&mut self.max_ttl_sum, weighted, "max_ttl_sum")?;
            add_metric(
                &mut self.max_ttl_count,
                observation.flow_count,
                "max_ttl_count",
            )?;
        }
        Ok(())
    }

    fn include(&mut self, child: &Self) -> Result<(), DomainError> {
        macro_rules! include_metrics {
            ($($field:ident),+ $(,)?) => {
                $(add_metric(&mut self.$field, child.$field, stringify!($field))?;)+
            };
        }
        include_metrics!(
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
        );
        Ok(())
    }
}

fn ensure_non_negative(value: i64, name: &'static str) -> Result<(), DomainError> {
    if value < 0 {
        Err(DomainError::NegativeMetric(name))
    } else {
        Ok(())
    }
}

fn add_metric(target: &mut i64, value: i64, name: &'static str) -> Result<(), DomainError> {
    ensure_non_negative(value, name)?;
    *target = target
        .checked_add(value)
        .ok_or(DomainError::MetricOverflow(name))?;
    Ok(())
}

fn multiply_metric(left: i64, right: i64, name: &'static str) -> Result<i64, DomainError> {
    ensure_non_negative(left, name)?;
    ensure_non_negative(right, "flow_count")?;
    left.checked_mul(right)
        .ok_or(DomainError::MetricOverflow(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedTraffic {
    pub scope: Scope,
    pub metrics: TrafficMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedProtocols {
    pub scope: Scope,
    pub protocols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedAddresses {
    pub scope: Scope,
    pub address_side: AddressSide,
    pub addresses: Vec<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedPorts {
    pub scope: Scope,
    pub port_side: PortSide,
    pub ports: FixedBitSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalBucket {
    pub key: BucketKey,
    pub traffic: Vec<ScopedTraffic>,
    pub protocols: Vec<ScopedProtocols>,
    pub addresses: Vec<ScopedAddresses>,
    pub ports: Vec<ScopedPorts>,
    pub five_minute_starts: BTreeSet<i64>,
}

impl CanonicalBucket {
    #[must_use]
    pub fn has_complete_five_minute_coverage(&self) -> bool {
        let expected = expected_five_minute_count(&self.key);
        i64::try_from(self.five_minute_starts.len()) == Ok(expected)
    }

    #[must_use]
    pub fn rows(&self) -> CanonicalRows<'_> {
        CanonicalRows {
            traffic_rows: self
                .traffic
                .iter()
                .map(|entry| TrafficRow {
                    key: self.key.clone(),
                    scope: entry.scope,
                    metrics: entry.metrics.clone(),
                    average_duration_ms: average(
                        entry.metrics.duration_sum_ms,
                        entry.metrics.duration_count,
                    ),
                    average_min_ttl: average(
                        entry.metrics.min_ttl_sum,
                        entry.metrics.min_ttl_count,
                    ),
                    average_max_ttl: average(
                        entry.metrics.max_ttl_sum,
                        entry.metrics.max_ttl_count,
                    ),
                })
                .collect(),
            protocol_rows: self
                .protocols
                .iter()
                .map(|entry| ProtocolRow {
                    key: self.key.clone(),
                    scope: entry.scope,
                    unique_protocols_count: entry.protocols.len(),
                    protocols_list: entry.protocols.join(","),
                })
                .collect(),
            address_count_rows: self
                .addresses
                .iter()
                .map(|entry| AddressCountRow {
                    key: self.key.clone(),
                    scope: entry.scope,
                    address_side: entry.address_side,
                    unique_address_count: entry.addresses.len(),
                })
                .collect(),
            port_count_rows: self
                .ports
                .iter()
                .flat_map(|entry| {
                    [PortRange::Low, PortRange::High].map(|port_range| PortCountRow {
                        key: self.key.clone(),
                        scope: entry.scope,
                        port_side: entry.port_side,
                        port_range,
                        unique_port_count: count_ports(&entry.ports, port_range),
                    })
                })
                .collect(),
            address_sets: self
                .addresses
                .iter()
                .map(|entry| AddressSetRow {
                    key: self.key.clone(),
                    scope: entry.scope,
                    address_side: entry.address_side,
                    addresses: &entry.addresses,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrafficRow {
    pub key: BucketKey,
    pub scope: Scope,
    pub metrics: TrafficMetrics,
    pub average_duration_ms: Option<f64>,
    pub average_min_ttl: Option<f64>,
    pub average_max_ttl: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolRow {
    pub key: BucketKey,
    pub scope: Scope,
    pub unique_protocols_count: usize,
    pub protocols_list: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressCountRow {
    pub key: BucketKey,
    pub scope: Scope,
    pub address_side: AddressSide,
    pub unique_address_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCountRow {
    pub key: BucketKey,
    pub scope: Scope,
    pub port_side: PortSide,
    pub port_range: PortRange,
    pub unique_port_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressSetRow<'a> {
    pub key: BucketKey,
    pub scope: Scope,
    pub address_side: AddressSide,
    pub addresses: &'a [IpAddr],
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalRows<'a> {
    pub traffic_rows: Vec<TrafficRow>,
    pub protocol_rows: Vec<ProtocolRow>,
    pub address_count_rows: Vec<AddressCountRow>,
    pub port_count_rows: Vec<PortCountRow>,
    pub address_sets: Vec<AddressSetRow<'a>>,
}

fn average(total: i64, count: i64) -> Option<f64> {
    (count != 0).then(|| total as f64 / count as f64)
}

fn count_ports(ports: &FixedBitSet, range: PortRange) -> usize {
    ports
        .ones()
        .filter(|port| match range {
            PortRange::Low => *port < 1024,
            PortRange::High => *port >= 1024,
        })
        .count()
}

/// Mutable builder for an immutable, deterministically ordered bucket.
#[derive(Debug, Clone)]
pub struct StatisticalBucket {
    key: BucketKey,
    traffic: BTreeMap<Scope, TrafficMetrics>,
    protocols: BTreeMap<Scope, BTreeSet<String>>,
    addresses: BTreeMap<(Scope, AddressSide), BTreeSet<IpAddr>>,
    ports: BTreeMap<(Scope, PortSide), FixedBitSet>,
    five_minute_starts: BTreeSet<i64>,
}

impl StatisticalBucket {
    #[must_use]
    pub fn new(key: BucketKey) -> Self {
        let mut five_minute_starts = BTreeSet::new();
        if key.granularity == Granularity::FiveMinutes {
            five_minute_starts.insert(key.bucket_start);
        }
        Self {
            key,
            traffic: BTreeMap::new(),
            protocols: BTreeMap::new(),
            addresses: BTreeMap::new(),
            ports: BTreeMap::new(),
            five_minute_starts,
        }
    }

    #[must_use]
    pub fn dense(key: BucketKey) -> Self {
        let mut bucket = Self::new(key);
        for ip_version in [IpVersion::V4, IpVersion::V6] {
            for (src_visibility, dst_visibility) in zero_fill_visibility_pairs() {
                let scope = Scope::new(ip_version, src_visibility, dst_visibility);
                bucket.traffic.insert(scope, TrafficMetrics::default());
                bucket.protocols.insert(scope, BTreeSet::new());
                for side in [AddressSide::Destination, AddressSide::Source] {
                    bucket.addresses.insert((scope, side), BTreeSet::new());
                    bucket.ports.insert((scope, side), empty_ports());
                }
            }
        }
        bucket
    }

    pub fn add(&mut self, fact: impl Into<StatisticalFact>) -> Result<(), DomainError> {
        match fact.into() {
            StatisticalFact::Observation(observation) => self.add_observation(observation),
            StatisticalFact::GroupedTraffic(grouped) => self.add_grouped(grouped),
            StatisticalFact::ScopedAddresses(addresses) => {
                self.add_scoped_addresses(addresses);
                Ok(())
            }
        }
    }

    pub fn include(&mut self, child: &CanonicalBucket) -> Result<(), DomainError> {
        let mut updates = Vec::with_capacity(child.traffic.len());
        for entry in &child.traffic {
            let mut metrics = self.traffic.get(&entry.scope).cloned().unwrap_or_default();
            metrics.include(&entry.metrics)?;
            updates.push((entry.scope, metrics));
        }
        for (scope, metrics) in updates {
            self.traffic.insert(scope, metrics);
        }
        for entry in &child.protocols {
            self.protocols
                .entry(entry.scope)
                .or_default()
                .extend(entry.protocols.iter().cloned());
        }
        for entry in &child.addresses {
            self.addresses
                .entry((entry.scope, entry.address_side))
                .or_default()
                .extend(entry.addresses.iter().copied());
        }
        for entry in &child.ports {
            self.ports
                .entry((entry.scope, entry.port_side))
                .or_insert_with(empty_ports)
                .union_with(&entry.ports);
        }
        self.five_minute_starts
            .extend(child.five_minute_starts.iter().copied());
        Ok(())
    }

    /// Whether every five-minute child in this bucket's interval was included.
    #[must_use]
    pub fn has_complete_five_minute_coverage(&self) -> bool {
        let expected = expected_five_minute_count(&self.key);
        i64::try_from(self.five_minute_starts.len()) == Ok(expected)
    }

    #[must_use]
    pub fn finish(&self) -> CanonicalBucket {
        CanonicalBucket {
            key: self.key.clone(),
            traffic: self
                .traffic
                .iter()
                .map(|(scope, metrics)| ScopedTraffic {
                    scope: *scope,
                    metrics: metrics.clone(),
                })
                .collect(),
            protocols: self
                .protocols
                .iter()
                .map(|(scope, protocols)| ScopedProtocols {
                    scope: *scope,
                    protocols: protocols.iter().cloned().collect(),
                })
                .collect(),
            addresses: self
                .addresses
                .iter()
                .map(|((scope, address_side), addresses)| ScopedAddresses {
                    scope: *scope,
                    address_side: *address_side,
                    addresses: addresses.iter().copied().collect(),
                })
                .collect(),
            ports: self
                .ports
                .iter()
                .map(|((scope, port_side), ports)| ScopedPorts {
                    scope: *scope,
                    port_side: *port_side,
                    ports: ports.clone(),
                })
                .collect(),
            five_minute_starts: self.five_minute_starts.clone(),
        }
    }

    fn add_observation(&mut self, observation: FlowObservation) -> Result<(), DomainError> {
        if IpVersion::of(observation.src_ip) != IpVersion::of(observation.dst_ip) {
            return Err(DomainError::MixedIpVersions);
        }
        ensure_non_negative(observation.flow_count, "flow_count")?;
        ensure_non_negative(observation.packets, "packets")?;
        ensure_non_negative(observation.bytes, "bytes")?;
        if let Some(duration) = observation.duration_ms {
            ensure_non_negative(duration, "duration_ms")?;
        }
        let scopes = scopes_for_tos(observation.ip_version(), observation.src_tos);
        let mut updates = Vec::with_capacity(scopes.len());
        for scope in scopes {
            let mut metrics = self.traffic.get(&scope).cloned().unwrap_or_default();
            metrics.add_observation(&observation)?;
            updates.push((scope, metrics));
        }
        for (scope, metrics) in updates {
            self.traffic.insert(scope, metrics);
            self.protocols
                .entry(scope)
                .or_default()
                .insert(observation.protocol.to_string());
            self.addresses
                .entry((scope, AddressSide::Source))
                .or_default()
                .insert(observation.src_ip);
            self.addresses
                .entry((scope, AddressSide::Destination))
                .or_default()
                .insert(observation.dst_ip);
            if let Some(port) = observation.src_port {
                insert_port(
                    self.ports
                        .entry((scope, AddressSide::Source))
                        .or_insert_with(empty_ports),
                    port,
                );
            }
            if let Some(port) = observation.dst_port {
                insert_port(
                    self.ports
                        .entry((scope, AddressSide::Destination))
                        .or_insert_with(empty_ports),
                    port,
                );
            }
        }
        Ok(())
    }

    fn add_grouped(&mut self, fact: GroupedTrafficFact) -> Result<(), DomainError> {
        let scopes = scopes_for_tos(fact.ip_version, fact.src_tos);
        let mut updates = Vec::with_capacity(scopes.len());
        for scope in scopes {
            let mut metrics = self.traffic.get(&scope).cloned().unwrap_or_default();
            metrics.add_traffic(fact.protocol, fact.flows, fact.packets, fact.bytes)?;
            updates.push((scope, metrics));
        }
        for (scope, metrics) in updates {
            self.traffic.insert(scope, metrics);
            self.protocols
                .entry(scope)
                .or_default()
                .insert(fact.protocol.to_string());
        }
        Ok(())
    }

    fn add_scoped_addresses(&mut self, fact: ScopedAddressesFact) {
        self.addresses
            .entry((fact.scope, fact.address_side))
            .or_default()
            .extend(fact.addresses);
    }
}

fn expected_five_minute_count(key: &BucketKey) -> i64 {
    let absolute_slots = (key.bucket_end - key.bucket_start) / 300;
    if key.granularity == Granularity::OneDay {
        // Capture filenames represent local wall-clock labels. A fall-back day
        // repeats absolute time but not filenames, while a spring-forward day
        // has fewer labels because its missing hour never occurs.
        absolute_slots.min(288)
    } else {
        absolute_slots
    }
}

fn empty_ports() -> FixedBitSet {
    FixedBitSet::new()
}

fn insert_port(ports: &mut FixedBitSet, port: u16) {
    if ports.len() < PORT_COUNT {
        ports.grow(PORT_COUNT);
    }
    ports.insert(usize::from(port));
}

fn zero_fill_visibility_pairs() -> [(Visibility, Visibility); 5] {
    [
        (Visibility::All, Visibility::All),
        (Visibility::Anonymized, Visibility::Anonymized),
        (Visibility::Anonymized, Visibility::Literal),
        (Visibility::Literal, Visibility::Anonymized),
        (Visibility::Literal, Visibility::Literal),
    ]
}

fn scopes_for_tos(ip_version: IpVersion, src_tos: u8) -> [Scope; 2] {
    let (source, destination) = exact_visibility_pair_from_tos(src_tos);
    [
        Scope::new(ip_version, Visibility::All, Visibility::All),
        Scope::new(ip_version, source.into(), destination.into()),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::net::{IpAddr, Ipv4Addr};

    use serde_json::json;

    use super::{
        AddressSide, BucketKey, DomainError, ExactVisibility, FlowObservation, FlowSelection,
        Granularity, GroupedTrafficFact, IpVersion, PortRange, Scope, ScopedAddressesFact,
        StatisticalBucket, Visibility,
    };

    fn address(value: [u8; 4]) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(value))
    }

    fn observation(src_ip: [u8; 4], dst_ip: [u8; 4], protocol: u8, src_tos: u8) -> FlowObservation {
        FlowObservation::new(address(src_ip), address(dst_ip), protocol, 1, 10, src_tos).unwrap()
    }

    fn key(granularity: Granularity, start: i64, end: i64) -> BucketKey {
        BucketKey::new("router", granularity, start, end)
    }

    #[test]
    fn selection_canonicalizes_prefix_and_combines_endpoint_and_visibility_filters() {
        let selection = FlowSelection::from_payload(Some(&json!({
            "ip_prefix": "192.0.2.99/24",
            "src_visibility": "literal",
            "dst_visibility": "anonymized"
        })))
        .unwrap();
        let matching = FlowObservation::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
            6,
            1,
            100,
            1,
        )
        .unwrap();
        let wrong_visibility =
            FlowObservation::new(matching.src_ip, matching.dst_ip, 6, 1, 100, 0).unwrap();

        assert!(selection.matches(&matching));
        assert!(!selection.matches(&wrong_visibility));
        assert_eq!(selection.src_visibility(), Some(ExactVisibility::Literal));
        assert_eq!(
            selection.nfdump_prefix_filter().as_deref(),
            Some("net 192.0.2.0/24")
        );
        assert_eq!(
            selection.normalized_payload(),
            json!({
                "version": 1,
                "kind": "flows",
                "ip_prefix": "192.0.2.0/24",
                "src_visibility": "literal",
                "dst_visibility": "anonymized"
            })
        );
    }

    #[test]
    fn selection_rejects_unknown_keys_and_invalid_all_criteria() {
        assert_eq!(
            FlowSelection::from_payload(Some(&json!({"ip_side": "source"}))),
            Err(DomainError::UnknownSelectionKeys(
                "[\"ip_side\"]".to_owned()
            ))
        );
        assert_eq!(
            FlowSelection::from_payload(Some(&json!({"kind": ""}))),
            Err(DomainError::InvalidSelectionKind)
        );
        assert_eq!(
            FlowSelection::from_payload(Some(&json!({
                "kind": "all",
                "src_visibility": "literal"
            }))),
            Err(DomainError::AllSelectionHasCriteria)
        );
    }

    #[test]
    fn dense_five_minute_bucket_renders_every_zero_scope() {
        let bucket = StatisticalBucket::dense(key(Granularity::FiveMinutes, 0, 300)).finish();
        let rows = bucket.rows();

        assert_eq!(bucket.traffic.len(), 10);
        assert_eq!(bucket.protocols.len(), 10);
        assert_eq!(bucket.addresses.len(), 20);
        assert_eq!(bucket.ports.len(), 20);
        assert_eq!(rows.port_count_rows.len(), 40);
        assert!(bucket.has_complete_five_minute_coverage());
        assert!(rows.traffic_rows.iter().all(|row| {
            row.metrics.flows == 0
                && row.average_duration_ms.is_none()
                && row.average_min_ttl.is_none()
                && row.average_max_ttl.is_none()
        }));
        assert_eq!(
            rows.traffic_rows
                .iter()
                .map(|row| (
                    row.scope.ip_version.number(),
                    row.scope.src_visibility.as_str(),
                    row.scope.dst_visibility.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (4, "all", "all"),
                (4, "anonymized", "anonymized"),
                (4, "anonymized", "literal"),
                (4, "literal", "anonymized"),
                (4, "literal", "literal"),
                (6, "all", "all"),
                (6, "anonymized", "anonymized"),
                (6, "anonymized", "literal"),
                (6, "literal", "anonymized"),
                (6, "literal", "literal"),
            ]
        );
    }

    #[test]
    fn rejected_overflow_leaves_bucket_unchanged() {
        let mut bucket = StatisticalBucket::new(key(Granularity::FiveMinutes, 0, 300));
        let overflowing = observation([192, 0, 2, 1], [198, 51, 100, 1], 6, 0)
            .with_measurements(Some(i64::MAX), None, None)
            .unwrap()
            .with_flow_count(2)
            .unwrap();

        assert_eq!(
            bucket.add(overflowing),
            Err(DomainError::MetricOverflow("duration_sum_ms"))
        );
        assert!(bucket.finish().traffic.is_empty());

        bucket
            .add(GroupedTrafficFact {
                ip_version: IpVersion::V4,
                protocol: 17,
                src_tos: 0,
                flows: i64::MAX,
                packets: 0,
                bytes: 0,
            })
            .unwrap();
        assert_eq!(
            bucket.add(GroupedTrafficFact {
                ip_version: IpVersion::V4,
                protocol: 17,
                src_tos: 0,
                flows: 1,
                packets: 0,
                bytes: 0,
            }),
            Err(DomainError::MetricOverflow("flows"))
        );
        assert_eq!(bucket.finish().traffic[0].metrics.flows, i64::MAX);
    }

    #[test]
    fn rollups_union_protocols_addresses_and_ports_independently_of_order() {
        fn child(
            start: i64,
            src: [u8; 4],
            protocol: u8,
            src_port: u16,
            dst_port: u16,
        ) -> super::CanonicalBucket {
            let mut child =
                StatisticalBucket::new(key(Granularity::FiveMinutes, start, start + 300));
            child
                .add(
                    observation(src, [198, 51, 100, 1], protocol, 2)
                        .with_ports(Some(src_port), Some(dst_port)),
                )
                .unwrap();
            child.finish()
        }

        let children = [
            child(0, [192, 0, 2, 2], 6, 0, 1023),
            child(300, [192, 0, 2, 1], 17, 1023, 1024),
            child(600, [192, 0, 2, 2], 58, 1024, 65535),
        ];
        let mut forward = StatisticalBucket::new(key(Granularity::ThirtyMinutes, 0, 1800));
        let mut reverse = StatisticalBucket::new(key(Granularity::ThirtyMinutes, 0, 1800));
        for child in &children {
            forward.include(child).unwrap();
        }
        for child in children.iter().rev() {
            reverse.include(child).unwrap();
        }
        let rolled_up = forward.finish();

        assert_eq!(rolled_up, reverse.finish());
        assert_eq!(rolled_up.five_minute_starts, BTreeSet::from([0, 300, 600]));
        assert!(!rolled_up.has_complete_five_minute_coverage());
        let all_scope = Scope::new(IpVersion::V4, Visibility::All, Visibility::All);
        assert_eq!(
            rolled_up
                .protocols
                .iter()
                .find(|entry| entry.scope == all_scope)
                .unwrap()
                .protocols,
            vec!["17", "58", "6"]
        );
        assert_eq!(
            rolled_up
                .addresses
                .iter()
                .find(|entry| {
                    entry.scope == all_scope && entry.address_side == AddressSide::Source
                })
                .unwrap()
                .addresses,
            vec![address([192, 0, 2, 1]), address([192, 0, 2, 2])]
        );
        let rows = rolled_up.rows();
        let counts = rows
            .port_count_rows
            .iter()
            .filter(|row| row.scope == all_scope)
            .map(|row| (row.port_side, row.port_range, row.unique_port_count))
            .collect::<Vec<_>>();
        assert_eq!(
            counts,
            vec![
                (AddressSide::Destination, PortRange::Low, 1),
                (AddressSide::Destination, PortRange::High, 2),
                (AddressSide::Source, PortRange::Low, 2),
                (AddressSide::Source, PortRange::High, 1),
            ]
        );
    }

    #[test]
    fn canonical_rows_preserve_weighting_missingness_and_scoped_address_unions() {
        let scope = Scope::new(IpVersion::V4, Visibility::Literal, Visibility::Anonymized);
        let mut bucket = StatisticalBucket::new(key(Granularity::FiveMinutes, 0, 300));
        bucket
            .add(
                observation([192, 0, 2, 1], [198, 51, 100, 1], 6, 1)
                    .with_measurements(Some(0), None, Some(10))
                    .unwrap(),
            )
            .unwrap();
        bucket
            .add(
                observation([192, 0, 2, 2], [198, 51, 100, 2], 6, 1)
                    .with_measurements(Some(100), Some(20), None)
                    .unwrap()
                    .with_flow_count(3)
                    .unwrap(),
            )
            .unwrap();
        bucket
            .add(ScopedAddressesFact::new(
                scope,
                AddressSide::Destination,
                [address([198, 51, 100, 3]), address([198, 51, 100, 2])],
            ))
            .unwrap();
        let bucket = bucket.finish();
        let rows = bucket.rows();
        let traffic = rows
            .traffic_rows
            .iter()
            .find(|row| row.scope == scope)
            .unwrap();

        assert_eq!(traffic.metrics.flows, 4);
        assert_eq!(traffic.metrics.duration_sum_ms, 300);
        assert_eq!(traffic.metrics.duration_count, 4);
        assert_eq!(traffic.average_duration_ms, Some(75.0));
        assert_eq!(traffic.average_min_ttl, Some(20.0));
        assert_eq!(traffic.average_max_ttl, Some(10.0));
        assert_eq!(
            rows.address_sets
                .iter()
                .find(|row| { row.scope == scope && row.address_side == AddressSide::Destination })
                .unwrap()
                .addresses,
            vec![
                address([198, 51, 100, 1]),
                address([198, 51, 100, 2]),
                address([198, 51, 100, 3]),
            ]
        );
    }
}
