"""Configuration and CLI helpers for NetFlow window extraction."""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from datetime import datetime
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo


REPO_ROOT = Path(__file__).resolve().parents[3]
NETFLOW_DB_TOOLS = Path(__file__).resolve().parents[1]
if str(NETFLOW_DB_TOOLS) not in sys.path:
    sys.path.insert(0, str(NETFLOW_DB_TOOLS))

GRANULARITIES = ("5m", "30m", "1h", "1d")
DEFAULT_DATASET_ID = "uoregon"
DEFAULT_START = "2025-06-01"
DEFAULT_END_EXCLUSIVE = "2026-06-01"
DEFAULT_OUTPUT_DIR_TEMPLATE = "data/{dataset}/extracts/{start}_to_{end}"
DEFAULT_TIMEZONE = os.environ.get("NETFLOW_TIMEZONE", "America/Los_Angeles")
SQLITE_FILENAME = "netflow.sqlite"
MANIFEST_FILENAME = "manifest.json"
OUTPUT_CHOICES = ("sqlite", "parquet")
DEFAULT_OUTPUTS = ("sqlite",)
SQLITE_SIDECAR_SUFFIXES = ("-wal", "-shm", "-journal")

TableConfig = dict[str, str]
TableManifest = dict[str, Any]


def table_config(
    *,
    time_column: str = "bucket_start",
    source_column: str = "source_id",
    granularity_column: str = "granularity",
) -> TableConfig:
    return {
        "time_column": time_column,
        "source_column": source_column,
        "granularity_column": granularity_column,
    }


TABLE_CONFIG = {
    "traffic_stats": table_config(),
    "protocol_stats": table_config(),
    "address_count_stats": table_config(),
    "port_count_stats": table_config(),
    "address_structure_stats": table_config(),
}

COMMON_STATS_COLUMNS = (
    "source_id",
    "granularity",
    "bucket_start",
    "bucket_end",
    "ip_version",
    "src_visibility",
    "dst_visibility",
    "processed_at",
)
REQUIRED_TABLE_COLUMNS = {
    "traffic_stats": (
        *COMMON_STATS_COLUMNS,
        "flows",
        "flows_tcp",
        "flows_udp",
        "flows_icmp",
        "flows_other",
        "packets",
        "packets_tcp",
        "packets_udp",
        "packets_icmp",
        "packets_other",
        "bytes",
        "bytes_tcp",
        "bytes_udp",
        "bytes_icmp",
        "bytes_other",
        "duration_sum_ms",
        "duration_count",
        "average_duration_ms",
        "min_ttl_sum",
        "min_ttl_count",
        "average_min_ttl",
        "max_ttl_sum",
        "max_ttl_count",
        "average_max_ttl",
    ),
    "protocol_stats": (
        *COMMON_STATS_COLUMNS,
        "unique_protocols_count",
        "protocols_list",
    ),
    "address_count_stats": (
        *COMMON_STATS_COLUMNS,
        "address_side",
        "unique_address_count",
    ),
    "port_count_stats": (
        *COMMON_STATS_COLUMNS,
        "port_side",
        "port_range",
        "unique_port_count",
    ),
    "address_structure_stats": (
        *COMMON_STATS_COLUMNS,
        "address_side",
        "structure_kind",
        "values_json",
        "metadata_json",
    ),
}


class OutputAction(argparse.Action):
    def __call__(
        self,
        parser: argparse.ArgumentParser,
        namespace: argparse.Namespace,
        values: str | Sequence[Any] | None,
        option_string: str | None = None,
    ) -> None:
        outputs = [] if not getattr(namespace, "_outputs_explicit", False) else list(self._current(namespace))
        outputs.append(str(values))
        setattr(namespace, self.dest, outputs)
        namespace._outputs_explicit = True

    def _current(self, namespace: argparse.Namespace) -> Sequence[str]:
        current = getattr(namespace, self.dest, None)
        return [] if current is None else current


def slug_path_part(value: str) -> str:
    slug = "".join(character if character.isalnum() or character in "_-" else "-" for character in value)
    return slug.strip("-") or "all"


def resolve_default_output_dir(
    *,
    dataset: str,
    start: str,
    end_exclusive: str,
    source_id: str | None,
) -> str:
    window = f"{slug_path_part(start)}_to_{slug_path_part(end_exclusive)}"
    if source_id is not None:
        window = f"{window}_source-{slug_path_part(source_id)}"
    return str(Path("data") / slug_path_part(dataset) / "extracts" / window)


def parse_boundary(value: str, timezone: str) -> datetime:
    raw = value.strip()
    if raw.isdigit():
        return datetime.fromtimestamp(int(raw), ZoneInfo(timezone))

    normalized = raw[:-1] + "+00:00" if raw.endswith("Z") else raw
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError:
        raise SystemExit(f"Invalid date/time boundary: {value!r}") from None

    if parsed.tzinfo is None:
        return parsed.replace(tzinfo=ZoneInfo(timezone))
    return parsed


def compute_window(start: str, end_exclusive: str, timezone: str) -> tuple[datetime, datetime, int, int]:
    start_dt = parse_boundary(start, timezone)
    end_dt = parse_boundary(end_exclusive, timezone)
    if end_dt <= start_dt:
        raise SystemExit("--end must be after --start.")
    return start_dt, end_dt, int(start_dt.timestamp()), int(end_dt.timestamp())


def resolve_path(path_value: str | Path) -> Path:
    path = Path(path_value).expanduser()
    return path.resolve() if path.is_absolute() else (REPO_ROOT / path).resolve()


def quote_identifier(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


def resolve_default_source_db(dataset_id: str) -> str:
    try:
        from common import get_dataset_db_path
    except ImportError as error:
        raise SystemExit(f"Could not import tools/netflow-db/common.py: {error}") from error
    except RuntimeError as error:
        raise SystemExit(f"Could not initialize dataset config: {error}") from error

    try:
        return str(get_dataset_db_path(dataset_id))
    except RuntimeError as error:
        raise SystemExit(f"Could not resolve default source DB: {error}") from error


def normalize_output_values(raw_outputs: Any) -> list[str]:
    if raw_outputs is None:
        values = list(DEFAULT_OUTPUTS)
    elif isinstance(raw_outputs, str):
        values = [raw_outputs]
    else:
        values = [str(output) for output in raw_outputs]

    outputs = list(dict.fromkeys(values))
    invalid = [output for output in outputs if output not in OUTPUT_CHOICES]
    if invalid:
        raise SystemExit(f"Invalid output selection(s): {', '.join(invalid)}")
    if not outputs:
        raise SystemExit("At least one output must be enabled.")
    return outputs


def selected_outputs(args: argparse.Namespace) -> list[str]:
    if hasattr(args, "output"):
        raise SystemExit("Use --output/outputs, not output.")
    return normalize_output_values(getattr(args, "outputs", None))


def output_modes(config: Any) -> set[str]:
    if hasattr(config, "output"):
        raise SystemExit("Use outputs, not output.")
    return set(normalize_output_values(getattr(config, "outputs", None)))


def resolve_parquet_dir(config: Any, output_dir: Path, outputs: set[str]) -> Path | None:
    if "parquet" not in outputs:
        return None
    parquet_dir = getattr(config, "parquet_dir", None)
    if parquet_dir:
        return resolve_path(parquet_dir)
    return output_dir / "parquet"
