#!/usr/bin/env python3
"""Backfill pipeline v2 NetFlow dashboard rollups."""

from __future__ import annotations

import argparse
import sqlite3
from pathlib import Path

from stats_v2 import backfill_netflow_stats_v2_rollup_rows, init_netflow_stats_v2_table


def main() -> None:
    parser = argparse.ArgumentParser(description='Backfill netflow_stats_v2 rollup rows.')
    parser.add_argument('db_path', type=Path)
    parser.add_argument('--analyze', action='store_true', help='Run ANALYZE after backfill.')
    args = parser.parse_args()

    if not args.db_path.is_file():
        raise SystemExit(f'Database not found: {args.db_path}')

    with sqlite3.connect(args.db_path) as conn:
        init_netflow_stats_v2_table(conn)
        with conn:
            backfill_netflow_stats_v2_rollup_rows(conn)
            if args.analyze:
                conn.execute('ANALYZE')
        count = conn.execute("SELECT COUNT(*) FROM netflow_stats_v2 WHERE granularity != '5m'").fetchone()[0]

    print(f'netflow_stats_v2_rollups={count}')


if __name__ == '__main__':
    main()
