"""Authoritative per-flow contract for row-oriented input adapters."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class FlowObservation:
    """A normalized flow with exact missing-value and millisecond semantics.

    Generic, indexed, Arrow CSV, and native nfcapd adapters produce this
    contract before the statistical bucket seam.
    """

    ip_version: int
    src_ip: str
    dst_ip: str
    protocol: int
    packets: int
    bytes_count: int
    src_tos: int
    time_received_ms: int | None = None
    time_end_ms: int | None = None
    time_start_ms: int | None = None
    src_port: int | None = None
    dst_port: int | None = None
    dst_tos: int = 0
    duration_ms: int | None = None
    min_ttl: int | None = None
    max_ttl: int | None = None
    flow_count: int = 1
