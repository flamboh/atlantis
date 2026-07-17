"""
Grouped nfdump readers for pipeline nfcapd inputs.

This avoids materializing every flow row in Python for native nfcapd files.
External CSV inputs still use the normalized row path.
"""

from __future__ import annotations

import csv
import ipaddress
import logging
import os
import re
import subprocess
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from flow_selection import FlowSelection
from statistical_bucket import (
    BucketKey,
    GroupedTrafficFact,
    Scope,
    ScopedAddressesFact,
    StatisticalBucket,
    visibility_pair_from_tos,
)
from stats import canonical_bucket_rows


NFDUMP_TIMEOUT_SECONDS = 300
PIPELINE_TIMEZONE = ZoneInfo(os.environ.get('NETFLOW_TIMEZONE', 'America/Los_Angeles'))
LOGGER = logging.getLogger(__name__)
NFCAPD_FILENAME_RE = re.compile(r'^nfcapd\.(\d{12})$')


def build_nfcapd_bucket_payload(
    path: str,
    source_id: str,
    selection: FlowSelection = FlowSelection(),
) -> dict:
    """Build 5m stats and raw aggregate sets for one nfcapd file."""
    bucket_start = parse_nfcapd_bucket_start(path)
    bucket_end = bucket_start + 300
    bucket = StatisticalBucket(
        BucketKey(source_id, '5m', bucket_start, bucket_end),
        dense=True,
    )
    for fact in read_scoped_address_facts(path, selection):
        bucket.add(fact)
    scoped_counters_by_version = {
        4: read_scoped_protocol_counters(path, 4, selection),
        6: read_scoped_protocol_counters(path, 6, selection),
    }
    for ip_version in (4, 6):
        for protocol, src_tos, packets, bytes_value, flows in scoped_counters_by_version[ip_version]:
            bucket.add(
                GroupedTrafficFact(
                    ip_version=ip_version,
                    protocol=protocol,
                    src_tos=src_tos,
                    flows=flows,
                    packets=packets,
                    bytes_count=bytes_value,
                )
            )
    canonical_bucket = bucket.finish()
    rows = canonical_bucket_rows(canonical_bucket)

    return {
        'processed_bucket': {
            'input_kind': 'nfcapd',
            'input_locator': path,
            'source_id': source_id,
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
        },
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'canonical_bucket': canonical_bucket,
    }


def is_nfcapd_bucket_filename(name: str) -> bool:
    """Return true for canonical nfcapd bucket filenames."""
    return NFCAPD_FILENAME_RE.fullmatch(name) is not None


def read_scoped_protocol_counters(
    path: str,
    ip_version: int,
    selection: FlowSelection = FlowSelection(),
) -> list[tuple[int, int, int, int, int]]:
    """Return `(protocol, src_tos, packets, bytes, flows)` scoped rows."""
    result = run_nfdump(
        [
            'nfdump',
            '-r',
            path,
            '-q',
            '-a',
            '-A',
            'proto,srctos',
            '-o',
            'csv:%pr,%stos,%pkt,%byt,%fl',
            *family_filter(ip_version, selection),
            '-N',
        ]
    )
    rows = []
    for row in csv.DictReader(result.stdout.splitlines()):
        if not row:
            continue
        if is_no_matching_flows_row(row):
            continue
        parsed = parse_scoped_protocol_counter_row(row, path=path, ip_version=ip_version)
        if parsed is not None and selection.allows_src_tos(parsed[1]):
            rows.append(parsed)
    return rows


def parse_scoped_protocol_counter_row(
    row: dict[str, str | None],
    *,
    path: str,
    ip_version: int,
) -> tuple[int, int, int, int, int] | None:
    """Parse one grouped scoped protocol row."""
    values: list[str] = []
    for key in ('proto', 'srcTos', 'packets', 'bytes', 'flows'):
        raw_value = row.get(key)
        if raw_value is None or raw_value.strip() == '':
            LOGGER.warning(
                'Skipping malformed nfdump scoped protocol row for %s ipv%s: %s',
                path,
                ip_version,
                row,
            )
            return None
        values.append(raw_value.strip())

    try:
        protocol, src_tos, packets, bytes_value, flows = (int(value) for value in values)
    except ValueError:
        LOGGER.warning(
            'Skipping malformed nfdump scoped protocol row for %s ipv%s: %s',
            path,
            ip_version,
            row,
        )
        return None

    return (protocol, src_tos, packets, bytes_value, flows)


def is_no_matching_flows_row(row: dict[str, str | None]) -> bool:
    """Return true when nfdump emits its no-match sentinel row in CSV mode."""
    return row.get('firstSeen') == 'No matching flows' and all(
        row.get(key) is None for key in ('duration', 'proto', 'packets', 'bytes', 'bps', 'bpp', 'flows')
    )


def read_scoped_address_facts(
    path: str,
    selection: FlowSelection = FlowSelection(),
) -> list[ScopedAddressesFact]:
    """Read typed scoped address facts for one nfcapd input."""
    result = run_nfdump(
        [
            'nfdump',
            '-r',
            path,
            '-q',
            '-a',
            '-A',
            'srcip,dstip,srctos',
            '-o',
            'csv:%sa,%da,%stos',
            *selection_filter(selection),
        ]
    )
    sets_by_key: dict[tuple[Scope, str], set[str | int]] = {}
    for row in csv.DictReader(result.stdout.splitlines()):
        source_ip = (row.get('srcAddr') or '').strip()
        destination_ip = (row.get('dstAddr') or '').strip()
        src_tos_raw = (row.get('srcTos') or '').strip()
        if not source_ip or not destination_ip or not src_tos_raw:
            continue
        try:
            src_tos = int(src_tos_raw)
        except ValueError:
            continue
        if not selection.allows_src_tos(src_tos):
            continue
        source_ipv4 = parse_ipv4_address(source_ip)
        destination_ipv4 = parse_ipv4_address(destination_ip)
        if source_ipv4 is not None and destination_ipv4 is not None:
            ip_version = 4
            source_value = source_ipv4
            destination_value = destination_ipv4
        elif ':' in source_ip and ':' in destination_ip:
            try:
                source_value = str(ipaddress.ip_address(source_ip))
                destination_value = str(ipaddress.ip_address(destination_ip))
            except ValueError:
                continue
            ip_version = 6
        else:
            continue

        exact_visibility = visibility_pair_from_tos(src_tos)
        for src_visibility, dst_visibility in (('all', 'all'), exact_visibility):
            scope = Scope(ip_version, src_visibility, dst_visibility)
            sets_by_key.setdefault((scope, 'source'), set()).add(source_value)
            sets_by_key.setdefault((scope, 'destination'), set()).add(destination_value)

    return [
        ScopedAddressesFact(scope, address_side, addresses)
        for (scope, address_side), addresses in sorted(sets_by_key.items())
    ]


def parse_ipv4_address(value: str) -> int | None:
    """Parse trusted dotted IPv4 text without constructing ipaddress objects."""
    if not value or '.' not in value:
        return None
    parts = value.split('.')
    if len(parts) != 4:
        return None
    address = 0
    for part in parts:
        if not part.isdigit():
            return None
        octet = int(part)
        if octet > 255:
            return None
        address = (address << 8) | octet
    return address


def run_nfdump(command: list[str]) -> subprocess.CompletedProcess[str]:
    """Run nfdump and raise on failure."""
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        timeout=NFDUMP_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        raise RuntimeError(f"nfdump failed: {result.stderr.strip()}")
    return result


def family_filter(
    ip_version: int,
    selection: FlowSelection = FlowSelection(),
) -> list[str]:
    """Return nfdump filter args for one IP family."""
    if ip_version == 4:
        family = 'ipv4'
        options = []
    elif ip_version == 6:
        family = 'ipv6'
        options = ['-6']
    else:
        raise ValueError('ip_version must be 4 or 6')
    prefix_filter = selection.nfdump_prefix_filter()
    expression = family if prefix_filter is None else f'{family} and {prefix_filter}'
    return [*options, expression]


def selection_filter(selection: FlowSelection) -> list[str]:
    """Return a native prefix predicate when selection can be pushed down."""
    prefix_filter = selection.nfdump_prefix_filter()
    return [] if prefix_filter is None else [prefix_filter]


def parse_nfcapd_bucket_start(path: str) -> int:
    """Parse the local-time 5m bucket from an nfcapd filename."""
    name = Path(path).name
    match = NFCAPD_FILENAME_RE.fullmatch(name)
    if match is None:
        raise ValueError(f'Invalid nfcapd filename: {name}')
    timestamp = match.group(1)
    local_time = datetime.strptime(timestamp, '%Y%m%d%H%M')
    # Ambiguous fall-back labels cannot distinguish both folds. Canonical nfcapd
    # paths contain one file per local 5m label, so use the first fold.
    return int(local_time.replace(tzinfo=PIPELINE_TIMEZONE).timestamp())
