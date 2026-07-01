"""Manifest helpers for NetFlow window extraction."""

from __future__ import annotations

import json
from collections.abc import Sequence
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .config import MANIFEST_FILENAME


def build_manifest(
    *,
    dataset_id: str,
    source_db: Path,
    output_dir: Path,
    sqlite_output_path: Path | None,
    parquet_dir: Path | None,
    start: str,
    end_exclusive: str,
    start_dt: datetime,
    end_dt: datetime,
    start_ts: int,
    end_ts: int,
    timezone: str,
    source_id: str | None,
    granularities: Sequence[str] | None,
    tables: dict[str, Any],
) -> dict[str, Any]:
    sqlite_path = None if sqlite_output_path is None else str(sqlite_output_path)
    parquet_path = None if parquet_dir is None else str(parquet_dir)
    granularities_value = None if granularities is None else list(granularities)
    return {
        "dataset_id": dataset_id,
        "source_db": str(source_db),
        "output_dir": str(output_dir),
        "sqlite_path": sqlite_path,
        "parquet_dir": parquet_path,
        "start": start_dt.isoformat(),
        "start_input": start,
        "start_ts": start_ts,
        "end_exclusive": end_dt.isoformat(),
        "end_exclusive_input": end_exclusive,
        "end_exclusive_ts": end_ts,
        "timezone": timezone,
        "source_id_filter": source_id,
        "granularity_filter": granularities_value,
        "generated_at": datetime.now(UTC).isoformat(),
        "filters": {
            "source_id": source_id,
            "granularities": granularities_value,
        },
        "window": {
            "start": start_dt.isoformat(),
            "start_input": start,
            "end_exclusive": end_dt.isoformat(),
            "end_exclusive_input": end_exclusive,
            "start_ts": start_ts,
            "end_exclusive_ts": end_ts,
            "timezone": timezone,
        },
        "outputs": {
            "source_db": str(source_db),
            "output_dir": str(output_dir),
            "sqlite_path": sqlite_path,
            "parquet_dir": parquet_path,
        },
        "tables": tables,
    }


def write_manifest(output_dir: Path, manifest: dict[str, Any]) -> Path:
    manifest_path = output_dir / MANIFEST_FILENAME
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return manifest_path
