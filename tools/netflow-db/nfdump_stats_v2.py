"""
Grouped nfdump readers for pipeline v2 nfcapd inputs.

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

from stats_v3 import (
    add_traffic_metrics_v3,
    address_set_entries_to_count_rows,
    empty_traffic_stats_row,
    protocol_set_entries_to_rows,
    visibility_pairs_for_row,
)


NFDUMP_TIMEOUT_SECONDS = 300
PIPELINE_TIMEZONE = ZoneInfo(os.environ.get('NETFLOW_TIMEZONE', 'America/Los_Angeles'))
LOGGER = logging.getLogger(__name__)
NFCAPD_FILENAME_RE = re.compile(r'^nfcapd\.(\d{12})$')


def build_nfcapd_bucket_payload(path: str, source_id: str) -> dict:
    """Build 5m stats and raw aggregate sets for one nfcapd file."""
    bucket_start = parse_nfcapd_bucket_start(path)
    bucket_end = bucket_start + 300
    address_v3_sets = read_address_sets_v3(path, source_id, bucket_start, bucket_end)
    scoped_counters_by_version = {
        4: read_scoped_protocol_counters(path, 4),
        6: read_scoped_protocol_counters(path, 6),
    }
    traffic_v3_rows, protocol_v3_sets = traffic_stats_rows_from_scoped_counters(
        scoped_counters_by_version,
        source_id,
        bucket_start,
        bucket_end,
    )

    return {
        'processed_bucket': {
            'input_kind': 'nfcapd',
            'input_locator': path,
            'source_id': source_id,
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
        },
        'traffic_v3_rows': traffic_v3_rows,
        'protocol_v3_rows': protocol_set_entries_to_rows(protocol_v3_sets),
        'address_count_v3_rows': address_set_entries_to_count_rows(address_v3_sets),
        'raw_bucket': {
            'source_id': source_id,
            'bucket_start': bucket_start,
            'traffic_v3_rows': traffic_v3_rows,
            'protocol_v3_sets': protocol_v3_sets,
            'address_v3_sets': address_v3_sets,
        },
    }


def is_nfcapd_bucket_filename(name: str) -> bool:
    """Return true for canonical nfcapd bucket filenames."""
    return NFCAPD_FILENAME_RE.fullmatch(name) is not None


def read_protocol_counters(path: str, ip_version: int) -> list[tuple[int, int, int, int]]:
    """Return `(protocol, packets, bytes, flows)` rows from grouped nfdump CSV."""
    result = run_nfdump(
        [
            'nfdump',
            '-r',
            path,
            '-q',
            '-a',
            '-A',
            'proto',
            '-o',
            'csv',
            *family_filter(ip_version),
            '-N',
        ]
    )
    rows = []
    for row in csv.DictReader(result.stdout.splitlines()):
        if not row:
            continue
        if is_no_matching_flows_row(row):
            continue
        parsed = parse_protocol_counter_row(row, path=path, ip_version=ip_version)
        if parsed is not None:
            rows.append(parsed)
    return rows


def read_traffic_stats_rows(
    path: str,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> tuple[list[dict], list[dict]]:
    """Read scoped v3 traffic rows and protocol sets from grouped nfdump CSV."""
    return traffic_stats_rows_from_scoped_counters(
        {
            4: read_scoped_protocol_counters(path, 4),
            6: read_scoped_protocol_counters(path, 6),
        },
        source_id,
        bucket_start,
        bucket_end,
    )


def traffic_stats_rows_from_scoped_counters(
    scoped_counters_by_version: dict[int, list[tuple[int, int, int, int, int]]],
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> tuple[list[dict], list[dict]]:
    """Build scoped v3 traffic rows from grouped protocol counters."""
    rows_by_key: dict[tuple[int, str, str], dict] = {}
    protocols_by_key: dict[tuple[int, str, str], set[str]] = {}

    for ip_version in (4, 6):
        for protocol, src_tos, packets, bytes_value, flows in scoped_counters_by_version[ip_version]:
            for src_visibility, dst_visibility in visibility_pairs_for_row(src_tos):
                key = (ip_version, src_visibility, dst_visibility)
                row = rows_by_key.setdefault(
                    key,
                    empty_traffic_stats_row(
                        source_id=source_id,
                        granularity='5m',
                        bucket_start=bucket_start,
                        bucket_end=bucket_end,
                        ip_version=ip_version,
                        src_visibility=src_visibility,
                        dst_visibility=dst_visibility,
                    ),
                )
                add_traffic_metrics_v3(
                    row,
                    protocol=protocol,
                    flows=flows,
                    packets=packets,
                    bytes_count=bytes_value,
                )
                protocols_by_key.setdefault(key, set()).add(str(protocol))

    protocol_entries = [
        {
            'source_id': source_id,
            'granularity': '5m',
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
            'ip_version': key[0],
            'src_visibility': key[1],
            'dst_visibility': key[2],
            'protocols': sorted(protocols),
        }
        for key, protocols in protocols_by_key.items()
    ]
    return (
        [rows_by_key[key] for key in sorted(rows_by_key)],
        sorted(protocol_entries, key=lambda row: (row['ip_version'], row['src_visibility'], row['dst_visibility'])),
    )


def read_scoped_protocol_counters(path: str, ip_version: int) -> list[tuple[int, int, int, int, int]]:
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
            *family_filter(ip_version),
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
        if parsed is not None:
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


def parse_protocol_counter_row(
    row: dict[str, str | None],
    *,
    path: str,
    ip_version: int,
) -> tuple[int, int, int, int] | None:
    """Parse one grouped protocol row, skipping sparse nfdump output rows."""
    values: list[str] = []
    for key in ('proto', 'packets', 'bytes', 'flows'):
        raw_value = row.get(key)
        if raw_value is None:
            LOGGER.warning(
                'Skipping malformed nfdump protocol row for %s ipv%s: %s',
                path,
                ip_version,
                row,
            )
            return None
        value = raw_value.strip()
        if not value:
            LOGGER.warning(
                'Skipping malformed nfdump protocol row for %s ipv%s: %s',
                path,
                ip_version,
                row,
            )
            return None
        values.append(value)

    try:
        protocol, packets, bytes_value, flows = (int(value) for value in values)
    except ValueError:
        LOGGER.warning(
            'Skipping malformed nfdump protocol row for %s ipv%s: %s',
            path,
            ip_version,
            row,
        )
        return None

    return (protocol, packets, bytes_value, flows)


def read_address_sets_by_version(path: str) -> tuple[set[str], set[str], set[str], set[str]]:
    """Read unique grouped source and destination address sets split by IP version."""
    result = run_nfdump(
        [
            'nfdump',
            '-r',
            path,
            '-q',
            '-a',
            '-A',
            'srcip,dstip',
            '-o',
            'fmt:%sa,%da',
        ]
    )
    source_ipv4 = set()
    destination_ipv4 = set()
    source_ipv6 = set()
    destination_ipv6 = set()
    for values in csv.reader(result.stdout.splitlines()):
        if len(values) < 2:
            continue
        source_ip = values[0].strip()
        destination_ip = values[1].strip()
        source_ipv4_value = parse_ipv4_address(source_ip)
        destination_ipv4_value = parse_ipv4_address(destination_ip)
        if source_ipv4_value is not None and destination_ipv4_value is not None:
            source_ipv4.add(source_ipv4_value)
            destination_ipv4.add(destination_ipv4_value)
            continue
        if ':' in source_ip and ':' in destination_ip:
            try:
                source_ipv6.add(str(ipaddress.ip_address(source_ip)))
                destination_ipv6.add(str(ipaddress.ip_address(destination_ip)))
            except ValueError:
                continue
    return source_ipv4, destination_ipv4, source_ipv6, destination_ipv6


def read_address_sets_v3(
    path: str,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> list[dict]:
    """Read scoped address sets for v3 address counts and structures."""
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
        ]
    )
    sets_by_key: dict[tuple[int, str, str, str], set] = {}
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

        for src_visibility, dst_visibility in visibility_pairs_for_row(src_tos):
            sets_by_key.setdefault(
                (ip_version, src_visibility, dst_visibility, 'source'),
                set(),
            ).add(source_value)
            sets_by_key.setdefault(
                (ip_version, src_visibility, dst_visibility, 'destination'),
                set(),
            ).add(destination_value)

    return [
        {
            'source_id': source_id,
            'granularity': '5m',
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
            'ip_version': key[0],
            'src_visibility': key[1],
            'dst_visibility': key[2],
            'address_side': key[3],
            'addresses': sorted(addresses),
        }
        for key, addresses in sorted(sets_by_key.items())
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


def looks_like_ipv4_address(value: str) -> bool:
    """Return true for trusted nfdump IPv4 address text."""
    return parse_ipv4_address(value) is not None


def read_address_sets(path: str, ip_version: int) -> tuple[set[str], set[str]]:
    """Read unique grouped source and destination address sets."""
    result = run_nfdump(
        [
            'nfdump',
            '-r',
            path,
            '-q',
            '-a',
            '-A',
            'srcip,dstip',
            '-o',
            'fmt:%sa,%da',
            *family_filter(ip_version),
        ]
    )
    source = set()
    destination = set()
    for values in csv.reader(result.stdout.splitlines()):
        if len(values) < 2:
            continue
        try:
            source.add(str(ipaddress.ip_address(values[0].strip())))
            destination.add(str(ipaddress.ip_address(values[1].strip())))
        except ValueError:
            continue
    return source, destination


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


def family_filter(ip_version: int) -> list[str]:
    """Return nfdump filter args for one IP family."""
    if ip_version == 4:
        return ['ipv4']
    if ip_version == 6:
        return ['ipv6', '-6']
    raise ValueError('ip_version must be 4 or 6')


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
