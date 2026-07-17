"""Canonical flow-selection semantics shared by every input adapter."""

from __future__ import annotations

import ipaddress
from dataclasses import dataclass
from functools import lru_cache
from typing import Any, Literal, Mapping, cast

from flow_observation import FlowObservation


ExactVisibility = Literal['literal', 'anonymized']
_VALID_VISIBILITIES = frozenset(('literal', 'anonymized'))


@dataclass(frozen=True, slots=True)
class FlowSelection:
    """Validated population predicate and adapter optimization hints."""

    ip_prefix: ipaddress.IPv4Network | ipaddress.IPv6Network | None = None
    src_visibility: ExactVisibility | None = None
    dst_visibility: ExactVisibility | None = None

    @classmethod
    def from_payload(cls, payload: Mapping[str, Any] | None) -> FlowSelection:
        if payload is None:
            return cls()
        if not isinstance(payload, Mapping):
            raise ValueError('selection must be an object')
        unknown = set(payload) - {
            'version',
            'kind',
            'ip_prefix',
            'src_visibility',
            'dst_visibility',
        }
        if unknown:
            raise ValueError(f'Unknown selection keys: {sorted(unknown)!r}')
        if payload.get('version', 1) != 1:
            raise ValueError('selection version must be 1')
        kind = payload.get('kind')
        if kind not in (None, 'all', 'flows'):
            raise ValueError("selection kind must be 'all' or 'flows'")
        if kind == 'all' and any(
            payload.get(key) not in (None, '')
            for key in ('ip_prefix', 'src_visibility', 'dst_visibility')
        ):
            raise ValueError("selection kind 'all' cannot define flow criteria")

        raw_prefix = payload.get('ip_prefix')
        try:
            prefix = (
                None
                if raw_prefix in (None, '')
                else ipaddress.ip_network(str(raw_prefix), strict=False)
            )
        except ValueError as error:
            raise ValueError(f'Invalid selection ip_prefix: {raw_prefix!r}') from error
        return cls(
            ip_prefix=prefix,
            src_visibility=_parse_visibility(payload.get('src_visibility'), 'src_visibility'),
            dst_visibility=_parse_visibility(payload.get('dst_visibility'), 'dst_visibility'),
        )

    @property
    def is_unrestricted(self) -> bool:
        return (
            self.ip_prefix is None
            and self.src_visibility is None
            and self.dst_visibility is None
        )

    def normalized_payload(self) -> dict[str, object]:
        if self.is_unrestricted:
            return {'version': 1, 'kind': 'all'}
        return {
            'version': 1,
            'kind': 'flows',
            'ip_prefix': None if self.ip_prefix is None else str(self.ip_prefix),
            'src_visibility': self.src_visibility,
            'dst_visibility': self.dst_visibility,
        }

    def matches(self, observation: FlowObservation) -> bool:
        if self.ip_prefix is not None:
            source = _parse_address(observation.src_ip)
            destination = _parse_address(observation.dst_ip)
            source_matches = source.version == self.ip_prefix.version and source in self.ip_prefix
            destination_matches = (
                destination.version == self.ip_prefix.version
                and destination in self.ip_prefix
            )
            if not source_matches and not destination_matches:
                return False
        return self.allows_src_tos(observation.src_tos)

    def allows_src_tos(self, src_tos: int) -> bool:
        bits = src_tos & 3
        source = 'anonymized' if bits & 2 else 'literal'
        destination = 'anonymized' if bits & 1 else 'literal'
        return (
            (self.src_visibility is None or self.src_visibility == source)
            and (self.dst_visibility is None or self.dst_visibility == destination)
        )

    def nfdump_prefix_filter(self) -> str | None:
        """Return a safe native filter built only from a parsed canonical network."""
        return None if self.ip_prefix is None else f'net {self.ip_prefix}'


def _parse_visibility(value: object, name: str) -> ExactVisibility | None:
    if value in (None, ''):
        return None
    if value not in _VALID_VISIBILITIES:
        raise ValueError(f"selection {name} must be 'literal' or 'anonymized'")
    return cast(ExactVisibility, value)


@lru_cache(maxsize=1_000_000)
def _parse_address(value: str) -> ipaddress.IPv4Address | ipaddress.IPv6Address:
    return ipaddress.ip_address(value)
