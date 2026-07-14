"""Canonical statistical bucket values and aggregation rules.

The module owns the meaning of a statistical bucket. Input adapters contribute
typed facts, orchestration includes completed child buckets, and downstream
adapters consume the immutable snapshot returned by ``finish``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Iterable, Literal


Granularity = Literal['5m', '30m', '1h', '1d']
Visibility = Literal['all', 'literal', 'anonymized']
AddressSide = Literal['source', 'destination']

ALL_VISIBILITY: tuple[Visibility, Visibility] = ('all', 'all')
EXACT_VISIBILITY_PAIRS: tuple[tuple[Visibility, Visibility], ...] = (
    ('anonymized', 'anonymized'),
    ('anonymized', 'literal'),
    ('literal', 'anonymized'),
    ('literal', 'literal'),
)
ZERO_FILL_VISIBILITY_PAIRS = (ALL_VISIBILITY, *EXACT_VISIBILITY_PAIRS)


@dataclass(frozen=True, order=True)
class BucketKey:
    source_id: str
    granularity: Granularity
    bucket_start: int
    bucket_end: int


@dataclass(frozen=True, order=True)
class Scope:
    ip_version: int
    src_visibility: Visibility
    dst_visibility: Visibility

    def __post_init__(self) -> None:
        if self.ip_version not in (4, 6):
            raise ValueError(f'Unsupported ip_version: {self.ip_version!r}')


@dataclass(frozen=True)
class FlowFact:
    ip_version: int
    src_ip: str
    dst_ip: str
    protocol: int
    packets: int
    bytes_count: int
    src_tos: int


@dataclass(frozen=True)
class GroupedTrafficFact:
    ip_version: int
    protocol: int
    src_tos: int
    flows: int
    packets: int
    bytes_count: int


@dataclass(frozen=True)
class ScopedAddressesFact:
    scope: Scope
    address_side: AddressSide
    addresses: Iterable[str | int]


@dataclass(frozen=True)
class TrafficMetrics:
    flows: int = 0
    flows_tcp: int = 0
    flows_udp: int = 0
    flows_icmp: int = 0
    flows_other: int = 0
    packets: int = 0
    packets_tcp: int = 0
    packets_udp: int = 0
    packets_icmp: int = 0
    packets_other: int = 0
    bytes: int = 0
    bytes_tcp: int = 0
    bytes_udp: int = 0
    bytes_icmp: int = 0
    bytes_other: int = 0


@dataclass(frozen=True)
class ScopedTraffic:
    scope: Scope
    metrics: TrafficMetrics


@dataclass(frozen=True)
class ScopedProtocols:
    scope: Scope
    protocols: tuple[str, ...]


@dataclass(frozen=True)
class ScopedAddresses:
    scope: Scope
    address_side: AddressSide
    addresses: tuple[str | int, ...]


@dataclass(frozen=True)
class CanonicalBucket:
    key: BucketKey
    traffic: tuple[ScopedTraffic, ...]
    protocols: tuple[ScopedProtocols, ...]
    addresses: tuple[ScopedAddresses, ...]
    five_minute_starts: frozenset[int]

    @property
    def has_complete_five_minute_coverage(self) -> bool:
        expected = (self.key.bucket_end - self.key.bucket_start) // 300
        return len(self.five_minute_starts) == expected


@dataclass
class _MutableMetrics:
    values: dict[str, int] = field(default_factory=lambda: {name: 0 for name in _METRIC_NAMES})

    def add(self, protocol: int, flows: int, packets: int, bytes_count: int) -> None:
        self.values['flows'] += flows
        self.values['packets'] += packets
        self.values['bytes'] += bytes_count
        suffix = _protocol_suffix(protocol)
        self.values[f'flows_{suffix}'] += flows
        self.values[f'packets_{suffix}'] += packets
        self.values[f'bytes_{suffix}'] += bytes_count

    def include(self, metrics: TrafficMetrics) -> None:
        for name in _METRIC_NAMES:
            self.values[name] += getattr(metrics, name)

    def finish(self) -> TrafficMetrics:
        return TrafficMetrics(**self.values)


_METRIC_NAMES = tuple(TrafficMetrics.__dataclass_fields__)


class StatisticalBucket:
    """Mutable builder for an immutable canonical statistical bucket."""

    def __init__(self, key: BucketKey, *, dense: bool = False) -> None:
        self.key = key
        self._traffic: dict[Scope, _MutableMetrics] = {}
        self._protocols: dict[Scope, set[str]] = {}
        self._addresses: dict[tuple[Scope, AddressSide], set[str | int]] = {}
        self._five_minute_starts: set[int] = set()
        if key.granularity == '5m':
            self._five_minute_starts.add(key.bucket_start)
        if dense:
            for ip_version in (4, 6):
                for src_visibility, dst_visibility in ZERO_FILL_VISIBILITY_PAIRS:
                    scope = Scope(ip_version, src_visibility, dst_visibility)
                    self._traffic[scope] = _MutableMetrics()
                    self._protocols[scope] = set()
                    for address_side in ('source', 'destination'):
                        self._addresses[(scope, address_side)] = set()

    def add(self, fact: FlowFact | GroupedTrafficFact | ScopedAddressesFact) -> None:
        if isinstance(fact, FlowFact):
            grouped = GroupedTrafficFact(
                ip_version=fact.ip_version,
                protocol=fact.protocol,
                src_tos=fact.src_tos,
                flows=1,
                packets=fact.packets,
                bytes_count=fact.bytes_count,
            )
            for scope in _scopes_for_tos(fact.ip_version, fact.src_tos):
                self._add_grouped(scope, grouped)
                self._addresses.setdefault((scope, 'source'), set()).add(fact.src_ip)
                self._addresses.setdefault((scope, 'destination'), set()).add(fact.dst_ip)
            return
        if isinstance(fact, GroupedTrafficFact):
            for scope in _scopes_for_tos(fact.ip_version, fact.src_tos):
                self._add_grouped(scope, fact)
            return
        if isinstance(fact, ScopedAddressesFact):
            self._addresses.setdefault((fact.scope, fact.address_side), set()).update(fact.addresses)
            return
        raise TypeError(f'Unsupported statistical fact: {type(fact).__name__}')

    def include(self, child: CanonicalBucket) -> None:
        for entry in child.traffic:
            self._traffic.setdefault(entry.scope, _MutableMetrics()).include(entry.metrics)
        for entry in child.protocols:
            self._protocols.setdefault(entry.scope, set()).update(entry.protocols)
        for entry in child.addresses:
            self._addresses.setdefault((entry.scope, entry.address_side), set()).update(entry.addresses)
        self._five_minute_starts.update(child.five_minute_starts)

    def finish(self) -> CanonicalBucket:
        return CanonicalBucket(
            key=self.key,
            traffic=tuple(
                ScopedTraffic(scope, self._traffic[scope].finish())
                for scope in sorted(self._traffic)
            ),
            protocols=tuple(
                ScopedProtocols(scope, tuple(sorted(self._protocols[scope])))
                for scope in sorted(self._protocols)
            ),
            addresses=tuple(
                ScopedAddresses(scope, side, tuple(sorted(addresses)))
                for (scope, side), addresses in sorted(self._addresses.items())
            ),
            five_minute_starts=frozenset(self._five_minute_starts),
        )

    def _add_grouped(self, scope: Scope, fact: GroupedTrafficFact) -> None:
        self._traffic.setdefault(scope, _MutableMetrics()).add(
            fact.protocol,
            fact.flows,
            fact.packets,
            fact.bytes_count,
        )
        self._protocols.setdefault(scope, set()).add(str(fact.protocol))


def visibility_pair_from_tos(src_tos: int) -> tuple[Visibility, Visibility]:
    return (
        'anonymized' if src_tos & 2 else 'literal',
        'anonymized' if src_tos & 1 else 'literal',
    )


def _scopes_for_tos(ip_version: int, src_tos: int) -> tuple[Scope, Scope]:
    src_visibility, dst_visibility = visibility_pair_from_tos(src_tos)
    return Scope(ip_version, 'all', 'all'), Scope(ip_version, src_visibility, dst_visibility)


def _protocol_suffix(protocol: int | str) -> str:
    protocol_value = int(protocol)
    if protocol_value == 6:
        return 'tcp'
    if protocol_value == 17:
        return 'udp'
    if protocol_value in (1, 58):
        return 'icmp'
    return 'other'
