#!/usr/bin/env python3
"""CLI for portable NetFlow window extraction."""

from __future__ import annotations

import argparse
from collections.abc import Sequence
from pathlib import Path

from .config import (
    DEFAULT_DATASET_ID,
    DEFAULT_END_EXCLUSIVE,
    DEFAULT_OUTPUT_DIR,
    DEFAULT_OUTPUT_DIR_TEMPLATE,
    DEFAULT_OUTPUTS,
    DEFAULT_START,
    DEFAULT_TIMEZONE,
    GRANULARITIES,
    MANIFEST_FILENAME,
    OUTPUT_CHOICES,
    SQLITE_FILENAME,
    SQLITE_SIDECAR_SUFFIXES,
    SQL_FILENAME,
    TABLE_CONFIG,
    TableConfig,
    TableManifest,
    OutputAction,
    compute_window,
    default_output_dir,
    normalize_output_values,
    output_modes,
    parse_boundary,
    resolve_default_output_dir,
    resolve_default_source_db,
    resolve_parquet_dir,
    resolve_path,
    selected_outputs,
    slug_path_part,
    table_config,
)
from .core import (
    ExtractionContext,
    ExtractionHelpers,
    collect_table_summary,
    describe_table_filter,
    extract as _core_extract,
    extract_table,
    normalized_granularities,
    optional_connection,
    print_dry_run,
    print_final_summary,
    print_plan,
    print_table_summary,
    resolved_paths,
)
from .manifest import build_manifest, write_manifest
from .parquet import (
    arrow_type_for,
    export_table_to_parquet,
    get_parquet_schema,
    parquet_modules,
)
from .publish import (
    PublishRecord,
    copy_path,
    path_exists,
    prepare_publish_source,
    publish_outputs,
    publish_outputs_with_helpers,
    publish_path,
    publish_sqlite_artifacts,
    remove_path,
    replace_path,
    rollback_publish,
)
from .sqlite import (
    build_filters,
    connect_db,
    create_source_snapshot,
    create_sqlite_table,
    copy_table_to_sqlite,
    get_schema_sql,
    get_table_column_types,
    iter_table_batches,
    managed_output_paths,
    quote_identifier,
    sqlite_artifact_paths,
    validate_parquet_dir,
    validate_required_tables,
    validate_source_db_managed_files,
)


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="extract_window",
        description="Extract a processed NetFlow window for analysis workflows.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )

    input_group = parser.add_argument_group("inputs")
    input_group.add_argument("--dataset", default=DEFAULT_DATASET_ID, help="Dataset id for default DB lookup.")
    input_group.add_argument("--source-db", help="Path to the source SQLite database.")

    window_group = parser.add_argument_group("window")
    window_group.add_argument("--start", default=DEFAULT_START, help="Inclusive start date/time.")
    window_group.add_argument(
        "--end",
        dest="end_exclusive",
        default=DEFAULT_END_EXCLUSIVE,
        help="Exclusive end date/time.",
    )
    window_group.add_argument("--timezone", default=DEFAULT_TIMEZONE, help="Timezone for naive boundaries.")

    filter_group = parser.add_argument_group("filters")
    filter_group.add_argument("--source-id", help="Only extract rows for one source_id.")
    filter_group.add_argument(
        "--granularity",
        action="append",
        choices=GRANULARITIES,
        help="Limit tables with granularity. Repeat for multiple granularities.",
    )

    output_group = parser.add_argument_group("outputs")
    output_group.add_argument(
        "--output",
        dest="outputs",
        action=OutputAction,
        choices=OUTPUT_CHOICES,
        default=list(DEFAULT_OUTPUTS),
        help="Output format to write. Repeat to enable multiple formats. Defaults to sqlite.",
    )
    output_group.add_argument(
        "--output-dir",
        default=argparse.SUPPRESS,
        help=(
            "Output directory. Default is generated as "
            f"{DEFAULT_OUTPUT_DIR_TEMPLATE}, with source suffix when filtered."
        ),
    )
    output_group.add_argument("--parquet-dir", help="Parquet directory. Defaults to <output-dir>/parquet.")
    output_group.add_argument("--dry-run", action="store_true", help="Print the resolved extraction plan without writing.")

    runtime_group = parser.add_argument_group("runtime")
    runtime_group.add_argument("--batch-size", type=int, default=5000, help="Fetch/insert batch size.")

    args = parser.parse_args(argv)
    args.outputs = selected_outputs(args)
    if hasattr(args, "_outputs_explicit"):
        delattr(args, "_outputs_explicit")
    if args.parquet_dir is not None and "parquet" not in args.outputs:
        raise SystemExit("--parquet-dir requires --output parquet.")
    if args.source_db is None:
        args.source_db = resolve_default_source_db(args.dataset)
    if not hasattr(args, "output_dir"):
        args.output_dir = resolve_default_output_dir(
            dataset=args.dataset,
            start=args.start,
            end_exclusive=args.end_exclusive,
            source_id=args.source_id,
        )
    if args.batch_size < 1:
        raise SystemExit("--batch-size must be positive.")
    return args


def _helpers() -> ExtractionHelpers:
    return ExtractionHelpers(
        build_filters=build_filters,
        create_source_snapshot=create_source_snapshot,
        create_sqlite_table=create_sqlite_table,
        copy_table_to_sqlite=copy_table_to_sqlite,
        export_table_to_parquet=export_table_to_parquet,
        publish_outputs=publish_outputs,
    )


def extract(args: argparse.Namespace, helpers: ExtractionHelpers | None = None) -> Path | None:
    return _core_extract(args, helpers or _helpers())


def main(argv: Sequence[str] | None = None) -> None:
    extract(parse_args(argv))


if __name__ == "__main__":
    main()
