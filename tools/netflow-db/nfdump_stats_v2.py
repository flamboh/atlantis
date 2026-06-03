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

from stats_v2 import protocol_metric_keys
from stats_v3 import (
    add_traffic_metrics_v3,
    address_set_entries_to_count_rows,
    empty_traffic_stats_v3_row,
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
    source_ipv4, destination_ipv4, source_ipv6, destination_ipv6 = read_address_sets_by_version(path)
    netflow_ipv4 = read_protocol_netflow_row(path, source_id, bucket_start, bucket_end, 4)
    netflow_ipv6 = read_protocol_netflow_row(path, source_id, bucket_start, bucket_end, 6)
    protocols_ipv4 = protocols_from_netflow_row(netflow_ipv4)
    protocols_ipv6 = protocols_from_netflow_row(netflow_ipv6)
    traffic_v3_rows, protocol_v3_sets = read_traffic_stats_v3_rows(
        path,
        source_id,
        bucket_start,
        bucket_end,
    )
    address_v3_sets = read_address_sets_v3(path, source_id, bucket_start, bucket_end)

    return {
        'processed_bucket': {
            'input_kind': 'nfcapd',
            'input_locator': path,
            'source_id': source_id,
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
        },
        'netflow_rows': [strip_internal_keys(netflow_ipv4), strip_internal_keys(netflow_ipv6)],
        'ip_row': {
            'source_id': source_id,
            'granularity': '5m',
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
            'sa_ipv4_count': len(source_ipv4),
            'da_ipv4_count': len(destination_ipv4),
            'sa_ipv6_count': len(source_ipv6),
            'da_ipv6_count': len(destination_ipv6),
        },
        'protocol_row': {
            'source_id': source_id,
            'granularity': '5m',
            'bucket_start': bucket_start,
            'bucket_end': bucket_end,
            'unique_protocols_count_ipv4': len(protocols_ipv4),
            'unique_protocols_count_ipv6': len(protocols_ipv6),
            'protocols_list_ipv4': ','.join(sorted(protocols_ipv4)),
            'protocols_list_ipv6': ','.join(sorted(protocols_ipv6)),
        },
        'traffic_v3_rows': traffic_v3_rows,
        'protocol_v3_rows': protocol_set_entries_to_rows(protocol_v3_sets),
        'address_count_v3_rows': address_set_entries_to_count_rows(address_v3_sets),
        'raw_bucket': {
            'source_id': source_id,
            'bucket_start': bucket_start,
            'source_ipv4': sorted(source_ipv4),
            'destination_ipv4': sorted(destination_ipv4),
            'source_ipv6': sorted(source_ipv6),
            'destination_ipv6': sorted(destination_ipv6),
            'protocols_ipv4': sorted(protocols_ipv4),
            'protocols_ipv6': sorted(protocols_ipv6),
            'maad_source_ipv4': sorted(source_ipv4),
            'maad_destination_ipv4': sorted(destination_ipv4),
            'netflow_rows': [strip_internal_keys(netflow_ipv4), strip_internal_keys(netflow_ipv6)],
            'traffic_v3_rows': traffic_v3_rows,
            'protocol_v3_sets': protocol_v3_sets,
            'address_v3_sets': address_v3_sets,
        },
    }


def is_nfcapd_bucket_filename(name: str) -> bool:
    """Return true for canonical nfcapd bucket filenames."""
    return NFCAPD_FILENAME_RE.fullmatch(name) is not None


def read_protocol_netflow_row(
    path: str,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
    ip_version: int,
) -> dict:
    """Read grouped protocol counters for one IP family."""
    row = empty_netflow_row(source_id, bucket_start, bucket_end, ip_version)
    for protocol, packets, bytes_value, flows in read_protocol_counters(path, ip_version):
        row['protocols'].add(str(protocol))
        row['flows'] += flows
        row['packets'] += packets
        row['bytes'] += bytes_value
        flow_key, packets_key, bytes_key = protocol_metric_keys(protocol)
        row[flow_key] += flows
        row[packets_key] += packets
        row[bytes_key] += bytes_value
    return row


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


def read_traffic_stats_v3_rows(
    path: str,
    source_id: str,
    bucket_start: int,
    bucket_end: int,
) -> tuple[list[dict], list[dict]]:
    """Read scoped v3 traffic rows and protocol sets from grouped nfdump CSV."""
    rows_by_key: dict[tuple[int, str, str], dict] = {}
    protocols_by_key: dict[tuple[int, str, str], set[str]] = {}

    for ip_version in (4, 6):
        for protocol, src_tos, packets, bytes_value, flows in read_scoped_protocol_counters(path, ip_version):
            for src_visibility, dst_visibility in visibility_pairs_for_row(src_tos):
                key = (ip_version, src_visibility, dst_visibility)
                row = rows_by_key.setdefault(
                    key,
                    empty_traffic_stats_v3_row(
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
            'proto,srctos,dsttos',
            '-o',
            'csv:%pr,%stos,%dtos,%pkt,%byt,%fl',
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
        if looks_like_ipv4_address(source_ip) and looks_like_ipv4_address(destination_ip):
            source_ipv4.add(source_ip)
            destination_ipv4.add(destination_ip)
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
            'srcip,dstip,srctos,dsttos',
            '-o',
            'csv:%sa,%da,%stos,%dtos',
        ]
    )
    sets_by_key: dict[tuple[int, str, str, str], set[str]] = {}
    for row in csv.DictReader(result.stdout.splitlines()):
        source_ip = (row.get('srcAddr') or '').strip()
        destination_ip = (row.get('dstAddr') or '').strip()
        src_tos_raw = (row.get('srcTos') or '').strip()
        if not source_ip or not destination_ip or not src_tos_raw:
            continue
        try:
            src_tos = int(src_tos_raw)
            source_addr = ipaddress.ip_address(source_ip)
            destination_addr = ipaddress.ip_address(destination_ip)
        except ValueError:
            continue
        if source_addr.version != destination_addr.version:
            continue
        ip_version = source_addr.version

        for src_visibility, dst_visibility in visibility_pairs_for_row(src_tos):
            sets_by_key.setdefault(
                (ip_version, src_visibility, dst_visibility, 'source'),
                set(),
            ).add(str(source_addr))
            sets_by_key.setdefault(
                (ip_version, src_visibility, dst_visibility, 'destination'),
                set(),
            ).add(str(destination_addr))

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


def looks_like_ipv4_address(value: str) -> bool:
    """Return true for trusted nfdump IPv4 address text."""
    return bool(value) and '.' in value and all(char.isdigit() or char == '.' for char in value)


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


def empty_netflow_row(source_id: str, bucket_start: int, bucket_end: int, ip_version: int) -> dict:
    """Create an empty netflow_stats_v2 row."""
    return {
        'source_id': source_id,
        'granularity': '5m',
        'bucket_start': bucket_start,
        'bucket_end': bucket_end,
        'ip_version': ip_version,
        'flows': 0,
        'flows_tcp': 0,
        'flows_udp': 0,
        'flows_icmp': 0,
        'flows_other': 0,
        'packets': 0,
        'packets_tcp': 0,
        'packets_udp': 0,
        'packets_icmp': 0,
        'packets_other': 0,
        'bytes': 0,
        'bytes_tcp': 0,
        'bytes_udp': 0,
        'bytes_icmp': 0,
        'bytes_other': 0,
        'protocols': set(),
    }


def protocols_from_netflow_row(row: dict) -> set[str]:
    """Infer protocol set from nonzero split counters."""
    return set(row['protocols'])


def strip_internal_keys(row: dict) -> dict:
    """Drop fields not persisted to netflow_stats_v2."""
    return {key: value for key, value in row.items() if key != 'protocols'}


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
