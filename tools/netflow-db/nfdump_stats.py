"""Streaming native nfdump adapter for complete per-flow observations."""

from __future__ import annotations

import csv
import functools
import ipaddress
import json
import logging
import os
import queue
import re
import signal
import shutil
import subprocess
import tempfile
import threading
import time
from collections.abc import Iterable, Iterator
from datetime import datetime
from pathlib import Path
from typing import BinaryIO, TextIO
from zoneinfo import ZoneInfo

from csv_ingest import CsvSourceConfigError
from flow_observation import FlowObservation
from flow_selection import FlowSelection
from nfdump_contract import (
    NFDUMP_REDUCER_CONTRACT_VERSION,
    NFDUMP_REDUCER_INPUT_CONTRACT,
    NFDUMP_REDUCER_OUTPUT_CONTRACT,
    NFDUMP_REDUCER_VERSION_LINE,
)
from normalized_rows import NFDUMP_CSV_FORMAT, normalize_nfdump_csv_values
from statistical_bucket import BucketKey, StatisticalBucket
from statistical_bucket import (
    CanonicalBucket,
    Scope,
    ScopedAddresses,
    ScopedPorts,
    ScopedProtocols,
    ScopedTraffic,
    TrafficMetrics,
)
from stats import canonical_bucket_rows


NFDUMP_TIMEOUT_SECONDS = 300
DEFAULT_NFDUMP_REDUCER_BIN = Path(__file__).resolve().parent / 'nfdump_reducer'
MAX_SQLITE_INTEGER = (1 << 63) - 1
MAX_DIAGNOSTIC_CHARACTERS = 64 * 1024
_REDUCER_SCOPE_KEYS = frozenset(
    {
        'ip_version',
        'src_visibility',
        'dst_visibility',
        'metrics',
        'protocols',
        'source_addresses',
        'destination_addresses',
        'source_ports_hex',
        'destination_ports_hex',
    }
)
_EXPECTED_SCOPES = frozenset(
    (ip_version, src_visibility, dst_visibility)
    for ip_version in (4, 6)
    for src_visibility, dst_visibility in (
        ('all', 'all'),
        ('anonymized', 'anonymized'),
        ('anonymized', 'literal'),
        ('literal', 'anonymized'),
        ('literal', 'literal'),
    )
)
PIPELINE_TIMEZONE = ZoneInfo(os.environ.get('NETFLOW_TIMEZONE', 'America/Los_Angeles'))
LOGGER = logging.getLogger(__name__)
NFCAPD_FILENAME_RE = re.compile(r'^nfcapd\.(\d{12})$')
_STREAM_END = object()


class NfdumpTimeoutError(TimeoutError):
    """Raised when the native reader exceeds its wall-clock deadline."""


def build_nfcapd_bucket_payload(
    path: str,
    source_id: str,
    selection: FlowSelection = FlowSelection(),
) -> dict:
    """Build one dense 5m bucket with the compiled all-family reducer."""
    bucket_start = parse_nfcapd_bucket_start(path)
    canonical_bucket = _canonical_bucket_from_reducer(
        _run_compiled_reducer(path, selection),
        BucketKey(source_id, '5m', bucket_start, bucket_start + 300),
    )
    rows = canonical_bucket_rows(canonical_bucket)
    return {
        'processed_bucket': {
            'input_kind': 'nfcapd',
            'input_locator': path,
            'source_id': source_id,
            'bucket_start': bucket_start,
            'bucket_end': bucket_start + 300,
        },
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'port_count_rows': rows['port_count_rows'],
        'canonical_bucket': canonical_bucket,
    }


def build_nfcapd_bucket_payload_python(
    path: str,
    source_id: str,
    selection: FlowSelection = FlowSelection(),
) -> dict:
    """Build a bucket with the slow row adapter retained as a correctness oracle."""
    bucket_start = parse_nfcapd_bucket_start(path)
    bucket = StatisticalBucket(
        BucketKey(source_id, '5m', bucket_start, bucket_start + 300),
        dense=True,
    )
    for observation in stream_nfdump_observations(path, source_id, selection):
        bucket.add(observation)
    canonical_bucket = bucket.finish()
    rows = canonical_bucket_rows(canonical_bucket)
    return {
        'processed_bucket': {
            'input_kind': 'nfcapd',
            'input_locator': path,
            'source_id': source_id,
            'bucket_start': bucket_start,
            'bucket_end': bucket_start + 300,
        },
        'traffic_rows': rows['traffic_rows'],
        'protocol_rows': rows['protocol_rows'],
        'address_count_rows': rows['address_count_rows'],
        'port_count_rows': rows['port_count_rows'],
        'canonical_bucket': canonical_bucket,
    }


def _run_compiled_reducer(path: str, selection: FlowSelection) -> dict:
    """Pipe one nfdump CSV stream into the versioned compiled reducer."""
    reducer_bin = _resolve_reducer_executable(
        os.environ.get('NFDUMP_REDUCER_BIN', str(DEFAULT_NFDUMP_REDUCER_BIN))
    )
    reducer_stat = reducer_bin.stat()
    _verify_reducer(
        str(reducer_bin.resolve()),
        reducer_stat.st_size,
        reducer_stat.st_mtime_ns,
    )
    nfdump_bin = _resolve_nfdump_executable(os.environ.get('NFDUMP_BIN', 'nfdump'))

    deadline = time.monotonic() + NFDUMP_TIMEOUT_SECONDS
    with (
        tempfile.TemporaryFile(mode='w+b') as nfdump_stderr,
        tempfile.TemporaryFile(mode='w+', encoding='utf-8') as reducer_stdout,
        tempfile.TemporaryFile(mode='w+b') as reducer_stderr,
    ):
        try:
            nfdump = subprocess.Popen(
                build_nfdump_command(path, selection, nfdump_bin=nfdump_bin),
                stdout=subprocess.PIPE,
                stderr=nfdump_stderr,
                text=True,
                start_new_session=True,
            )
        except OSError as error:
            raise RuntimeError(f'Unable to start nfdump executable: {nfdump_bin}') from error
        assert nfdump.stdout is not None
        try:
            reducer = subprocess.Popen(
                build_reducer_command(reducer_bin, selection),
                stdin=nfdump.stdout,
                stdout=reducer_stdout,
                stderr=reducer_stderr,
                text=True,
                start_new_session=True,
            )
        except BaseException as error:
            nfdump.stdout.close()
            _terminate_process_group(nfdump)
            if isinstance(error, OSError):
                raise RuntimeError(
                    f'Unable to start nfdump reducer executable: {reducer_bin}'
                ) from error
            raise
        nfdump.stdout.close()
        try:
            _wait_for_process(reducer, deadline)
            _wait_for_process(nfdump, deadline)
        except BaseException as error:
            _terminate_process_group(reducer)
            _terminate_process_group(nfdump)
            diagnostics = _pipeline_diagnostics(
                nfdump,
                nfdump_stderr,
                reducer,
                reducer_stderr,
            )
            if isinstance(error, NfdumpTimeoutError):
                raise NfdumpTimeoutError(
                    f'nfdump/reducer pipeline exceeded {NFDUMP_TIMEOUT_SECONDS}s; '
                    f'{diagnostics}'
                ) from error
            raise

        if reducer.returncode != 0 or nfdump.returncode != 0:
            raise RuntimeError(
                'nfdump/reducer pipeline failed: '
                + _pipeline_diagnostics(
                    nfdump,
                    nfdump_stderr,
                    reducer,
                    reducer_stderr,
                )
            )
        reducer_stdout.seek(0)
        try:
            value = json.load(reducer_stdout, object_pairs_hook=_strict_json_object)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise RuntimeError('nfdump reducer emitted malformed JSON') from error
    if not isinstance(value, dict):
        raise RuntimeError('nfdump reducer payload must be an object')
    return value


def _resolve_reducer_executable(raw_path: str) -> Path:
    path = Path(raw_path).expanduser()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise RuntimeError(
            f'nfdump reducer is unavailable or not executable: {path}; '
            'run scripts/build_nfdump_reducer.sh'
        )
    return path.resolve()


def _resolve_nfdump_executable(command: str) -> str:
    resolved = shutil.which(command)
    if resolved is None:
        raise RuntimeError(f'nfdump executable is unavailable: {command}')
    return resolved


def _strict_json_object(pairs: list[tuple[str, object]]) -> dict:
    value = {}
    for key, item in pairs:
        if key in value:
            raise RuntimeError(f'nfdump reducer emitted duplicate JSON field: {key}')
        value[key] = item
    return value


def _read_process_stderr(stream: BinaryIO) -> str:
    stream.seek(0, os.SEEK_END)
    end = stream.tell()
    start = max(0, end - MAX_DIAGNOSTIC_CHARACTERS)
    stream.seek(start)
    detail = stream.read().decode('utf-8', errors='replace').strip()
    if start:
        detail = f'[truncated to final {MAX_DIAGNOSTIC_CHARACTERS} characters] {detail}'
    return detail or 'no stderr'


def _pipeline_diagnostics(
    nfdump: subprocess.Popen[str],
    nfdump_stderr: BinaryIO,
    reducer: subprocess.Popen[str],
    reducer_stderr: BinaryIO,
) -> str:
    return (
        f'nfdump exit={nfdump.returncode!r}, stderr={_read_process_stderr(nfdump_stderr)!r}; '
        f'reducer exit={reducer.returncode!r}, stderr={_read_process_stderr(reducer_stderr)!r}'
    )


@functools.lru_cache(maxsize=8)
def _verify_reducer(path: str, size: int, mtime_ns: int) -> None:
    """Fail closed unless the executable reports the exact compiled contract."""
    del size, mtime_ns
    try:
        result = subprocess.run(
            [path, '--version'],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f'Unable to verify nfdump reducer: {path}') from error
    if (
        result.returncode != 0
        or result.stdout.strip() != NFDUMP_REDUCER_VERSION_LINE
        or result.stderr
    ):
        raise RuntimeError(f'Unsupported nfdump reducer executable: {path}')


def build_reducer_command(reducer_bin: Path, selection: FlowSelection) -> list[str]:
    """Build reducer arguments for independent visibility selection."""
    command = [
        str(reducer_bin),
        '--contract-version',
        str(NFDUMP_REDUCER_CONTRACT_VERSION),
        '--input-contract',
        NFDUMP_REDUCER_INPUT_CONTRACT,
        '--output-contract',
        NFDUMP_REDUCER_OUTPUT_CONTRACT,
    ]
    if selection.src_visibility is not None:
        command.extend(('--src-visibility', selection.src_visibility))
    if selection.dst_visibility is not None:
        command.extend(('--dst-visibility', selection.dst_visibility))
    return command


def _canonical_bucket_from_reducer(value: dict, key: BucketKey) -> CanonicalBucket:
    """Validate the native helper contract and restore the canonical snapshot."""
    if value.get('version') != NFDUMP_REDUCER_CONTRACT_VERSION:
        raise RuntimeError('Unsupported nfdump reducer payload version')
    if value.get('input_contract') != NFDUMP_REDUCER_INPUT_CONTRACT:
        raise RuntimeError('Unsupported nfdump reducer input contract')
    if value.get('output_contract') != NFDUMP_REDUCER_OUTPUT_CONTRACT:
        raise RuntimeError('Unsupported nfdump reducer output contract')
    if set(value) != {'version', 'input_contract', 'output_contract', 'scopes'}:
        raise RuntimeError('Malformed nfdump reducer payload fields')
    scopes = value.get('scopes')
    if not isinstance(scopes, list) or len(scopes) != 10:
        raise RuntimeError('nfdump reducer payload must contain 10 dense scopes')

    traffic = []
    protocols = []
    addresses = []
    ports = []
    seen_scopes = set()
    for raw_scope in scopes:
        if not isinstance(raw_scope, dict):
            raise RuntimeError('nfdump reducer scope must be an object')
        if set(raw_scope) != _REDUCER_SCOPE_KEYS:
            raise RuntimeError('Malformed nfdump reducer scope fields')
        try:
            ip_version = raw_scope['ip_version']
            src_visibility = raw_scope['src_visibility']
            dst_visibility = raw_scope['dst_visibility']
            if type(ip_version) is not int:
                raise TypeError('ip_version must be an integer')
            if not isinstance(src_visibility, str) or not isinstance(dst_visibility, str):
                raise TypeError('visibility must be a string')
            scope = Scope(ip_version, src_visibility, dst_visibility)
            metrics = raw_scope['metrics']
            raw_protocols = raw_scope['protocols']
            source_addresses = raw_scope['source_addresses']
            destination_addresses = raw_scope['destination_addresses']
        except (KeyError, TypeError, ValueError) as error:
            raise RuntimeError('Malformed nfdump reducer scope') from error
        scope_key = (scope.ip_version, scope.src_visibility, scope.dst_visibility)
        if scope_key not in _EXPECTED_SCOPES:
            raise RuntimeError('Unexpected nfdump reducer scope')
        if scope_key in seen_scopes:
            raise RuntimeError('Duplicate nfdump reducer scope')
        seen_scopes.add(scope_key)
        if not isinstance(metrics, list) or len(metrics) != len(TrafficMetrics.__dataclass_fields__):
            raise RuntimeError('Malformed nfdump reducer metrics')
        if not all(type(metric) is int and 0 <= metric <= MAX_SQLITE_INTEGER for metric in metrics):
            raise RuntimeError('Malformed nfdump reducer metric value')
        _validate_metrics(metrics)
        if not isinstance(raw_protocols, list) or not all(
            isinstance(protocol, str) for protocol in raw_protocols
        ):
            raise RuntimeError('Malformed nfdump reducer protocols')
        if raw_protocols != sorted(set(raw_protocols)) or any(
            not protocol.isdigit()
            or int(protocol) > 255
            or str(int(protocol)) != protocol
            for protocol in raw_protocols
        ):
            raise RuntimeError('Non-canonical nfdump reducer protocols')
        if not isinstance(source_addresses, list) or not all(
            isinstance(address, str) for address in source_addresses
        ):
            raise RuntimeError('Malformed nfdump reducer source addresses')
        if source_addresses != sorted(set(source_addresses)):
            raise RuntimeError('Non-canonical nfdump reducer source addresses')
        _validate_addresses(source_addresses, scope.ip_version, 'source')
        if not isinstance(destination_addresses, list) or not all(
            isinstance(address, str) for address in destination_addresses
        ):
            raise RuntimeError('Malformed nfdump reducer destination addresses')
        if destination_addresses != sorted(set(destination_addresses)):
            raise RuntimeError('Non-canonical nfdump reducer destination addresses')
        _validate_addresses(destination_addresses, scope.ip_version, 'destination')
        traffic.append(ScopedTraffic(scope, TrafficMetrics(*metrics)))
        protocols.append(ScopedProtocols(scope, tuple(raw_protocols)))
        addresses.extend(
            (
                ScopedAddresses(scope, 'source', tuple(source_addresses)),
                ScopedAddresses(scope, 'destination', tuple(destination_addresses)),
            )
        )
        ports.extend(
            (
                ScopedPorts(scope, 'source', _parse_bitmap(raw_scope, 'source_ports_hex')),
                ScopedPorts(
                    scope,
                    'destination',
                    _parse_bitmap(raw_scope, 'destination_ports_hex'),
                ),
            )
        )
    if seen_scopes != _EXPECTED_SCOPES:
        raise RuntimeError('Incomplete nfdump reducer scopes')
    return CanonicalBucket(
        key=key,
        traffic=tuple(sorted(traffic, key=lambda entry: entry.scope)),
        protocols=tuple(sorted(protocols, key=lambda entry: entry.scope)),
        addresses=tuple(
            sorted(addresses, key=lambda entry: (entry.scope, entry.address_side))
        ),
        ports=tuple(sorted(ports, key=lambda entry: (entry.scope, entry.port_side))),
        five_minute_starts=frozenset((key.bucket_start,)),
    )


def _parse_bitmap(raw_scope: dict, name: str) -> int:
    value = raw_scope.get(name)
    if not isinstance(value, str) or not value or len(value) > 16384:
        raise RuntimeError(f'Malformed nfdump reducer {name}')
    try:
        bitmap = int(value, 16)
    except ValueError as error:
        raise RuntimeError(f'Malformed nfdump reducer {name}') from error
    if bitmap.bit_length() > 65536:
        raise RuntimeError(f'Malformed nfdump reducer {name}')
    if value != format(bitmap, 'x'):
        raise RuntimeError(f'Non-canonical nfdump reducer {name}')
    return bitmap


def _validate_metrics(metrics: list[int]) -> None:
    for offset in (0, 5, 10):
        if metrics[offset] != sum(metrics[offset + 1 : offset + 5]):
            raise RuntimeError('Inconsistent nfdump reducer protocol metrics')
    flows = metrics[0]
    duration_sum, duration_count = metrics[15:17]
    min_ttl_sum, min_ttl_count = metrics[17:19]
    max_ttl_sum, max_ttl_count = metrics[19:21]
    if duration_count != flows:
        raise RuntimeError('Inconsistent nfdump reducer duration count')
    if min_ttl_count > flows or max_ttl_count > flows:
        raise RuntimeError('Inconsistent nfdump reducer TTL count')
    if min_ttl_sum > min_ttl_count * 255 or max_ttl_sum > max_ttl_count * 255:
        raise RuntimeError('Inconsistent nfdump reducer TTL sum')
    if duration_count == 0 and duration_sum != 0:
        raise RuntimeError('Inconsistent nfdump reducer duration sum')


def _validate_addresses(addresses: list[str], ip_version: int, side: str) -> None:
    for address in addresses:
        try:
            parsed = ipaddress.ip_address(address)
        except ValueError as error:
            raise RuntimeError(f'Malformed nfdump reducer {side} address') from error
        if parsed.version != ip_version or str(parsed) != address:
            raise RuntimeError(f'Non-canonical nfdump reducer {side} address')


def stream_nfdump_observations(
    path: str,
    source_id: str,
    selection: FlowSelection = FlowSelection(),
) -> Iterator[FlowObservation]:
    """Yield selected observations without buffering nfdump stdout or stderr."""
    command = build_nfdump_command(path, selection)
    deadline = time.monotonic() + NFDUMP_TIMEOUT_SECONDS
    with tempfile.TemporaryFile(mode='w+', encoding='utf-8') as stderr:
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=True,
            bufsize=1,
        )
        assert process.stdout is not None
        lines: queue.Queue[str | BaseException | object] = queue.Queue(maxsize=1024)
        cancelled = threading.Event()
        reader = threading.Thread(
            target=_read_stdout,
            args=(process.stdout, lines, cancelled),
            daemon=True,
        )
        reader.start()
        try:
            for values in csv.reader(_iter_stdout_lines(lines, deadline)):
                if not values or _is_header_or_no_match(values):
                    continue
                try:
                    row = normalize_nfdump_csv_values(values, source_id)
                except CsvSourceConfigError as error:
                    raise RuntimeError(f'Malformed nfdump CSV row for {path}: {error}') from error
                if selection.matches(row.observation):
                    yield row.observation
            _wait_for_process(process, deadline)
        except BaseException:
            cancelled.set()
            _terminate_process(process)
            process.stdout.close()
            reader.join(timeout=1)
            raise
        finally:
            cancelled.set()
            process.stdout.close()
            reader.join(timeout=1)

        if process.returncode != 0:
            stderr.seek(0)
            detail = stderr.read().strip()
            raise RuntimeError(
                f'nfdump failed with exit code {process.returncode}: {detail or "no stderr"}'
            )


def build_nfdump_command(
    path: str,
    selection: FlowSelection = FlowSelection(),
    *,
    nfdump_bin: str = 'nfdump',
) -> list[str]:
    """Build one all-family command with safe prefix pushdown."""
    command = [nfdump_bin, '-r', path, '-q', '-o', NFDUMP_CSV_FORMAT, '-N']
    native_filter = selection.nfdump_prefix_filter()
    return command if native_filter is None else [*command, native_filter]


def _read_stdout(
    stdout: TextIO,
    lines: queue.Queue[str | BaseException | object],
    cancelled: threading.Event,
) -> None:
    try:
        for line in stdout:
            if not _put_stream_item(lines, line, cancelled):
                return
    except BaseException as error:
        _put_stream_item(lines, error, cancelled)
    finally:
        _put_stream_item(lines, _STREAM_END, cancelled)


def _put_stream_item(
    lines: queue.Queue[str | BaseException | object],
    item: str | BaseException | object,
    cancelled: threading.Event,
) -> bool:
    while not cancelled.is_set():
        try:
            lines.put(item, timeout=0.1)
            return True
        except queue.Full:
            continue
    return False


def _iter_stdout_lines(
    lines: queue.Queue[str | BaseException | object],
    deadline: float,
) -> Iterable[str]:
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise NfdumpTimeoutError('nfdump timed out while streaming CSV output')
        try:
            item = lines.get(timeout=remaining)
        except queue.Empty as error:
            raise NfdumpTimeoutError('nfdump timed out while streaming CSV output') from error
        if item is _STREAM_END:
            return
        if isinstance(item, BaseException):
            raise RuntimeError('Failed while reading nfdump stdout') from item
        assert isinstance(item, str)
        yield item


def _wait_for_process(process: subprocess.Popen[str], deadline: float) -> None:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise NfdumpTimeoutError('nfdump timed out after streaming CSV output')
    try:
        process.wait(timeout=remaining)
    except subprocess.TimeoutExpired as error:
        raise NfdumpTimeoutError('nfdump timed out after streaming CSV output') from error


def _terminate_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.kill()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        LOGGER.error('Process did not exit within 5s after SIGKILL: pid=%s', process.pid)


def _terminate_process_group(process: subprocess.Popen[str]) -> None:
    """Kill a session leader and any descendants, then reap the direct child."""
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    if process.poll() is not None:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        LOGGER.error('Process group did not exit within 5s after SIGKILL: pid=%s', process.pid)


def _is_header_or_no_match(values: list[str]) -> bool:
    first = values[0].strip().lower()
    return first in {
        'trr',
        'firstseen',
        'received',
        'time received',
        'time_received',
        'no matching flows',
    }


def is_nfcapd_bucket_filename(name: str) -> bool:
    """Return true for canonical nfcapd bucket filenames."""
    return NFCAPD_FILENAME_RE.fullmatch(name) is not None


def parse_nfcapd_bucket_start(path: str) -> int:
    """Parse the local-time 5m bucket from an nfcapd filename."""
    name = Path(path).name
    match = NFCAPD_FILENAME_RE.fullmatch(name)
    if match is None:
        raise ValueError(f'Invalid nfcapd filename: {name}')
    local_time = datetime.strptime(match.group(1), '%Y%m%d%H%M')
    return int(local_time.replace(tzinfo=PIPELINE_TIMEZONE).timestamp())
