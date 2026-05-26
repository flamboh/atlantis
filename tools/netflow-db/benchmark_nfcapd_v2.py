#!/usr/bin/env python3
"""Benchmark pipeline-v2 nfcapd ingest slices."""

from __future__ import annotations

import argparse
import json
import sqlite3
import tempfile
import time
from collections import defaultdict
from pathlib import Path
from typing import Callable

import nfdump_stats_v2
import pipeline_v2


def main() -> None:
    parser = argparse.ArgumentParser(description='Benchmark pipeline-v2 nfcapd ingest.')
    parser.add_argument('--path', action='append', default=[], help='nfcapd file path. Repeatable.')
    parser.add_argument('--source-id', help='Source id for --path inputs.')
    parser.add_argument('--root', help='Canonical nfcapd tree root.')
    parser.add_argument('--day', help='Day for --root discovery, YYYY-MM-DD.')
    parser.add_argument('--source', action='append', default=[], help='Source id for --root discovery. Repeatable.')
    parser.add_argument('--limit', type=int, help='Limit discovered/input files.')
    parser.add_argument('--repeat', type=int, default=1, help='Repeat count.')
    parser.add_argument('--mode', choices=('payload', 'pipeline'), default='payload')
    parser.add_argument('--database-path', help='SQLite path for pipeline mode. Defaults to a temp database.')
    parser.add_argument('--max-workers', type=int, default=1)
    parser.add_argument('--maad-bin', default=str(pipeline_v2.DEFAULT_MAAD_BIN))
    parser.add_argument('--maad-backend', default='subprocess')
    parser.add_argument('--run-maad', action='store_true')
    parser.add_argument(
        '--legacy-address-split',
        action='store_true',
        help='Benchmark the previous two-command IPv4/IPv6 address extraction shape.',
    )
    args = parser.parse_args()

    phase_stats = PhaseTimingStats()
    specs = phase_stats.time_call('discovery', lambda: build_specs(args))
    if args.limit is not None:
        specs = specs[: args.limit]
    if not specs:
        raise SystemExit('no nfcapd inputs selected')

    stats = NfdumpTimingStats()
    patches = [
        ModulePatch(nfdump_stats_v2, 'run_nfdump', stats.wrap(nfdump_stats_v2.run_nfdump)),
        ModulePatch(
            pipeline_v2,
            'build_nfcapd_bucket_payload',
            phase_stats.wrap('nfcapd_payload', pipeline_v2.build_nfcapd_bucket_payload),
        ),
        ModulePatch(
            pipeline_v2,
            'write_input_payload',
            phase_stats.wrap('sqlite_input_write', pipeline_v2.write_input_payload),
        ),
        ModulePatch(
            pipeline_v2,
            'write_aggregate_rows',
            phase_stats.wrap('aggregate_generation_and_write', pipeline_v2.write_aggregate_rows),
        ),
        ModulePatch(
            pipeline_v2,
            'flush_streaming_aggregate_buckets',
            phase_stats.wrap('aggregate_generation_and_write', pipeline_v2.flush_streaming_aggregate_buckets),
        ),
        ModulePatch(
            pipeline_v2,
            'process_maad_row_task',
            phase_stats.wrap('maad_task', pipeline_v2.process_maad_row_task),
        ),
    ]
    original_read_address_sets_by_version = nfdump_stats_v2.read_address_sets_by_version
    for patch in patches:
        patch.apply()
    if args.legacy_address_split:
        nfdump_stats_v2.read_address_sets_by_version = read_address_sets_by_version_legacy
    try:
        summary = run_benchmark(args, specs, stats, phase_stats)
    finally:
        for patch in reversed(patches):
            patch.restore()
        nfdump_stats_v2.read_address_sets_by_version = original_read_address_sets_by_version

    print(json.dumps(summary, indent=2, sort_keys=True))


def build_specs(args: argparse.Namespace) -> list[dict]:
    if args.path:
        if not args.source_id:
            raise SystemExit('--source-id is required with --path')
        return [
            {
                'input_kind': 'nfcapd',
                'path': path,
                'source_id': args.source_id,
            }
            for path in args.path
        ]

    if args.root:
        if not args.day:
            raise SystemExit('--day is required with --root')
        if not args.source:
            raise SystemExit('--source is required with --root')
        return pipeline_v2.discover_nfcapd_tree_specs(
            args.root,
            args.source,
            pipeline_v2.parse_config_date(args.day),
        )

    raise SystemExit('provide --path or --root')


class NfdumpTimingStats:
    def __init__(self) -> None:
        self.calls = 0
        self.seconds = 0.0
        self.by_label: dict[str, dict[str, float | int]] = defaultdict(lambda: {'calls': 0, 'seconds': 0.0})

    def wrap(self, run_nfdump: Callable):
        def timed_run_nfdump(command: list[str]):
            label = nfdump_command_label(command)
            start = time.perf_counter()
            try:
                return run_nfdump(command)
            finally:
                elapsed = time.perf_counter() - start
                self.calls += 1
                self.seconds += elapsed
                self.by_label[label]['calls'] += 1
                self.by_label[label]['seconds'] += elapsed

        return timed_run_nfdump

    def snapshot(self) -> dict:
        return {
            'calls': self.calls,
            'seconds': round(self.seconds, 6),
            'by_label': {
                label: {
                    'calls': values['calls'],
                    'seconds': round(values['seconds'], 6),
                }
                for label, values in sorted(self.by_label.items())
            },
        }


class PhaseTimingStats:
    def __init__(self) -> None:
        self.by_label: dict[str, dict[str, float | int]] = defaultdict(lambda: {'calls': 0, 'seconds': 0.0})

    def wrap(self, label: str, function: Callable):
        def timed_function(*args, **kwargs):
            return self.time_call(label, lambda: function(*args, **kwargs))

        return timed_function

    def time_call(self, label: str, callback: Callable):
        start = time.perf_counter()
        try:
            return callback()
        finally:
            elapsed = time.perf_counter() - start
            self.by_label[label]['calls'] += 1
            self.by_label[label]['seconds'] += elapsed

    def snapshot(self) -> dict:
        return {
            label: {
                'calls': values['calls'],
                'seconds': round(values['seconds'], 6),
            }
            for label, values in sorted(self.by_label.items())
        }


class ModulePatch:
    def __init__(self, module, name: str, replacement) -> None:
        self.module = module
        self.name = name
        self.replacement = replacement
        self.original = getattr(module, name)

    def apply(self) -> None:
        setattr(self.module, self.name, self.replacement)

    def restore(self) -> None:
        setattr(self.module, self.name, self.original)


def nfdump_command_label(command: list[str]) -> str:
    joined = ' '.join(command)
    if '-A proto' in joined:
        if 'ipv4' in command:
            return 'protocol_ipv4'
        if 'ipv6' in command:
            return 'protocol_ipv6'
        return 'protocol'
    if '-A srcip,dstip' in joined:
        if 'ipv4' in command:
            return 'address_ipv4'
        if 'ipv6' in command:
            return 'address_ipv6'
        return 'address_all'
    return 'other'


def read_address_sets_by_version_legacy(path: str) -> tuple[set[str], set[str], set[str], set[str]]:
    source_ipv4, destination_ipv4 = nfdump_stats_v2.read_address_sets(path, 4)
    source_ipv6, destination_ipv6 = nfdump_stats_v2.read_address_sets(path, 6)
    return source_ipv4, destination_ipv4, source_ipv6, destination_ipv6


def run_benchmark(
    args: argparse.Namespace,
    specs: list[dict],
    stats: NfdumpTimingStats,
    phase_stats: PhaseTimingStats,
) -> dict:
    start = time.perf_counter()
    if args.mode == 'payload':
        payloads = 0
        for _ in range(args.repeat):
            for spec in specs:
                pipeline_v2.build_input_payload(
                    spec,
                    args.maad_bin,
                    maad_backend=args.maad_backend,
                    run_maad=args.run_maad,
                )
                payloads += 1
        row_counts = {'payloads': payloads}
    else:
        row_counts = run_pipeline_benchmark(args, specs)
    elapsed = time.perf_counter() - start
    return {
        'mode': args.mode,
        'legacy_address_split': args.legacy_address_split,
        'inputs': len(specs),
        'repeat': args.repeat,
        'elapsed_seconds': round(elapsed, 6),
        'seconds_per_input': round(elapsed / (len(specs) * args.repeat), 6),
        'row_counts': row_counts,
        'phases': phase_stats.snapshot(),
        'nfdump': stats.snapshot(),
    }


def run_pipeline_benchmark(args: argparse.Namespace, specs: list[dict]) -> dict[str, int]:
    if args.database_path:
        conn = sqlite3.connect(args.database_path)
        try:
            return run_pipeline_iterations(args, specs, conn)
        finally:
            conn.close()

    with tempfile.TemporaryDirectory(prefix='nfcapd-v2-bench-') as tmp_dir:
        conn = sqlite3.connect(str(Path(tmp_dir) / 'netflow.sqlite'))
        try:
            return run_pipeline_iterations(args, specs, conn)
        finally:
            conn.close()


def run_pipeline_iterations(args: argparse.Namespace, specs: list[dict], conn: sqlite3.Connection) -> dict[str, int]:
    for _ in range(args.repeat):
        pipeline_v2.process_input_specs(
            conn,
            specs,
            maad_bin=args.maad_bin,
            maad_backend=args.maad_backend,
            max_workers=args.max_workers,
            run_maad=args.run_maad,
        )
    return {
        'processed_inputs_v2': table_count(conn, 'processed_inputs_v2'),
        'netflow_stats_v2': table_count(conn, 'netflow_stats_v2'),
        'netflow_stats_aggregate_v2': table_count(conn, 'netflow_stats_aggregate_v2'),
        'ip_stats_v2': table_count(conn, 'ip_stats_v2'),
        'protocol_stats_v2': table_count(conn, 'protocol_stats_v2'),
        'structure_stats_v2': table_count(conn, 'structure_stats_v2'),
    }


def table_count(conn: sqlite3.Connection, table_name: str) -> int:
    return int(conn.execute(f'SELECT COUNT(*) FROM {table_name}').fetchone()[0])


if __name__ == '__main__':
    main()
