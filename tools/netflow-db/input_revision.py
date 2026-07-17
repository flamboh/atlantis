"""Exact, canonical input revisions for pipeline provenance."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from csv_ingest import CsvSourceConfig


CSV_DECODER_VERSION = 1
NFCAPD_DECODER_VERSION = 2
GAP_DECODER_VERSION = 1


def canonical_json(value: Any) -> str:
    """Encode a JSON value deterministically for persistence and hashing."""
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True)


def fingerprint(value: Any) -> str:
    """Return a SHA-256 fingerprint of a canonical JSON value."""
    return hashlib.sha256(canonical_json(value).encode()).hexdigest()


def file_sha256(path: str | Path) -> str:
    """Hash a file exactly with bounded memory."""
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


class InputContentChangedError(RuntimeError):
    """Raised when an input changes during revision capture or decoding."""


@dataclass(frozen=True, slots=True)
class FileSnapshot:
    """Cheap file identity used to detect changes after exact hashing."""

    device: int
    inode: int
    size: int
    mtime_ns: int
    ctime_ns: int

    @classmethod
    def capture(cls, path: str | Path) -> FileSnapshot:
        stat = Path(path).stat()
        return cls(
            device=stat.st_dev,
            inode=stat.st_ino,
            size=stat.st_size,
            mtime_ns=stat.st_mtime_ns,
            ctime_ns=stat.st_ctime_ns,
        )


@dataclass(frozen=True, slots=True)
class ExpectedAbsence:
    """Path whose continued absence is required for a synthetic gap."""

    path: str

    @classmethod
    def capture(cls, path: str | Path) -> ExpectedAbsence:
        snapshot = cls(str(path))
        snapshot.verify()
        return snapshot

    def verify(self) -> None:
        if os.path.lexists(self.path):
            raise InputContentChangedError(
                f'Expected absent input appeared before gap publication: {self.path!r}'
            )


def capture_file_revision(path: str | Path) -> tuple[str, FileSnapshot]:
    """Hash a stable file once and return its post-hash identity."""
    before = FileSnapshot.capture(path)
    content_fingerprint = file_sha256(path)
    after = FileSnapshot.capture(path)
    if before != after:
        raise InputContentChangedError(
            f'Input changed while its revision was being captured: {str(path)!r}'
        )
    return content_fingerprint, after


@dataclass(frozen=True, slots=True)
class InputRevision:
    """Exact content and decoder identity for one input locator."""

    input_kind: str
    locator: str
    content_fingerprint: str
    decoder_fingerprint: str
    fingerprint: str

    @classmethod
    def create(
        cls,
        *,
        input_kind: str,
        locator: str,
        content_fingerprint: str,
        decoder_fingerprint: str,
    ) -> InputRevision:
        revision_fingerprint = fingerprint(
            {
                'version': 1,
                'input_kind': input_kind,
                'locator': locator,
                'content_fingerprint': content_fingerprint,
                'decoder_fingerprint': decoder_fingerprint,
            }
        )
        return cls(
            input_kind=input_kind,
            locator=locator,
            content_fingerprint=content_fingerprint,
            decoder_fingerprint=decoder_fingerprint,
            fingerprint=revision_fingerprint,
        )


def csv_decoder_fingerprint(config: CsvSourceConfig) -> str:
    """Fingerprint validated CSV decoding semantics, not mapping-file formatting."""
    return fingerprint(
        {
            'version': CSV_DECODER_VERSION,
            'kind': 'csv',
            'config': {
                'delimiter': config.delimiter,
                'has_header': config.has_header,
                'timestamp_format': config.timestamp_format,
                'datetime_format': config.datetime_format,
                'timestamp_timezone': config.timestamp_timezone,
                'fieldnames': config.fieldnames,
                'columns': config.columns,
                'protocol_map': config.protocol_map,
                'source_id_value': config.source_id_value,
                'source_id_column': config.source_id_column,
                'skip_bad_column_count': config.skip_bad_column_count,
                'archive_member_contains': config.archive_member_contains,
            },
        }
    )


def csv_input_revision(path: str | Path, config: CsvSourceConfig) -> InputRevision:
    revision, _snapshot = capture_csv_input_revision(path, config)
    return revision


def capture_csv_input_revision(
    path: str | Path,
    config: CsvSourceConfig,
) -> tuple[InputRevision, FileSnapshot]:
    locator = str(path)
    content_fingerprint, snapshot = capture_file_revision(path)
    return (
        InputRevision.create(
            input_kind='csv',
            locator=locator,
            content_fingerprint=content_fingerprint,
            decoder_fingerprint=csv_decoder_fingerprint(config),
        ),
        snapshot,
    )


def nfcapd_decoder_fingerprint() -> str:
    """Fingerprint the streaming per-observation native decoder contract."""
    return fingerprint(
        {
            'version': NFCAPD_DECODER_VERSION,
            'kind': 'nfcapd-streaming-observations',
            'fields': [
                'timestamps',
                'addresses',
                'ports',
                'protocol',
                'packets',
                'bytes',
                'tos',
                'flow-count',
                'min-ttl',
                'max-ttl',
            ],
        }
    )


def nfcapd_input_revision(path: str | Path) -> InputRevision:
    revision, _snapshot = capture_nfcapd_input_revision(path)
    return revision


def capture_nfcapd_input_revision(
    path: str | Path,
) -> tuple[InputRevision, FileSnapshot]:
    locator = str(path)
    content_fingerprint, snapshot = capture_file_revision(path)
    return (
        InputRevision.create(
            input_kind='nfcapd',
            locator=locator,
            content_fingerprint=content_fingerprint,
            decoder_fingerprint=nfcapd_decoder_fingerprint(),
        ),
        snapshot,
    )


def gap_input_revision(input_kind: str, locator: str) -> InputRevision:
    """Return the stable revision of a synthetic empty input."""
    return InputRevision.create(
        input_kind=input_kind,
        locator=locator,
        content_fingerprint=fingerprint({'version': 1, 'kind': 'empty-gap'}),
        decoder_fingerprint=fingerprint(
            {'version': GAP_DECODER_VERSION, 'kind': f'{input_kind}-gap'}
        ),
    )


def revision_for_locator(revision: InputRevision, locator: str) -> InputRevision:
    """Retarget an owning revision to a derived locator without changing semantics."""
    if locator == revision.locator:
        return revision
    return InputRevision.create(
        input_kind=revision.input_kind,
        locator=locator,
        content_fingerprint=revision.content_fingerprint,
        decoder_fingerprint=revision.decoder_fingerprint,
    )


def verify_file_snapshot(path: str | Path, snapshot: FileSnapshot) -> None:
    """Verify that a file's identity stayed stable after exact hashing."""
    if FileSnapshot.capture(path) != snapshot:
        raise InputContentChangedError(
            f'Input changed while it was being decoded: {str(path)!r}'
        )
