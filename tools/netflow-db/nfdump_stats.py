"""Streaming native nfdump adapter for complete per-flow observations."""

from __future__ import annotations

import csv
import logging
import os
import queue
import re
import subprocess
import tempfile
import threading
import time
from collections.abc import Iterable, Iterator
from datetime import datetime
from pathlib import Path
from typing import TextIO
from zoneinfo import ZoneInfo

from csv_ingest import CsvSourceConfigError
from flow_observation import FlowObservation
from flow_selection import FlowSelection
from normalized_rows import NFDUMP_CSV_FORMAT, normalize_nfdump_csv_values
from statistical_bucket import BucketKey, StatisticalBucket
from stats import canonical_bucket_rows


NFDUMP_TIMEOUT_SECONDS = 300
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
    """Build one dense 5m bucket from one all-family streaming nfdump pass."""
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


def build_nfdump_command(path: str, selection: FlowSelection = FlowSelection()) -> list[str]:
    """Build one all-family command with safe prefix pushdown."""
    command = ['nfdump', '-r', path, '-q', '-o', NFDUMP_CSV_FORMAT]
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
    process.wait()


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
