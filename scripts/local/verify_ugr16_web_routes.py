#!/usr/bin/env python3
"""Verify UGR16 through the running apps/web API routes."""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


DEFAULT_BASE_URLS = ('http://localhost:5173', 'http://localhost:4173')
PACIFIC = ZoneInfo('America/Los_Angeles')


def main() -> None:
    parser = argparse.ArgumentParser(description='Verify UGR16 web API routes.')
    parser.add_argument('--db-path', default='data/ugr16/netflow.sqlite')
    parser.add_argument('--dataset', default='ugr16')
    parser.add_argument('--source-id', default='ugr16')
    parser.add_argument('--base-url', default=os.environ.get('WEB_BASE_URL'))
    args = parser.parse_args()

    db_path = Path(args.db_path)
    if not db_path.is_file():
        raise SystemExit(f'Database not found: {db_path}')

    window = select_verification_window(db_path, args.source_id)
    base_url = select_base_url(args.base_url)
    verify_routes(base_url, args.dataset, args.source_id, window)
    print(f'OK web routes {base_url} dataset={args.dataset} window={window.start}..{window.end}')


class VerificationWindow:
    def __init__(self, *, start: int, end: int, detail_bucket: int) -> None:
        self.start = start
        self.end = end
        self.detail_bucket = detail_bucket


def select_verification_window(db_path: Path, source_id: str) -> VerificationWindow:
    with sqlite3.connect(db_path) as conn:
        detail_row = conn.execute(
            """
            SELECT ts.bucket_start
            FROM traffic_stats ts
            JOIN address_structure_stats st
              ON st.source_id = ts.source_id
             AND st.granularity = '5m'
             AND st.bucket_start = ts.bucket_start
             AND st.ip_version = 4
             AND st.src_visibility = 'all'
             AND st.dst_visibility = 'all'
             AND st.structure_kind = 'structure'
            JOIN address_structure_stats sp
              ON sp.source_id = ts.source_id
             AND sp.granularity = '5m'
             AND sp.bucket_start = ts.bucket_start
             AND sp.ip_version = 4
             AND sp.src_visibility = 'all'
             AND sp.dst_visibility = 'all'
             AND sp.structure_kind = 'spectrum'
            WHERE ts.source_id = ?
              AND ts.granularity = '5m'
              AND ts.src_visibility = 'all'
              AND ts.dst_visibility = 'all'
            ORDER BY ts.bucket_start
            LIMIT 1
            """,
            (source_id,),
        ).fetchone()
        if detail_row is None:
            raise SystemExit(f'No 5m MAAD detail bucket for source_id={source_id}')

    detail_bucket = int(detail_row[0])
    return VerificationWindow(
        start=detail_bucket,
        end=detail_bucket + 3600,
        detail_bucket=detail_bucket,
    )


def select_base_url(configured: str | None) -> str:
    candidates = (configured,) if configured else DEFAULT_BASE_URLS
    last_error: Exception | None = None
    for base_url in candidates:
        if not base_url:
            continue
        try:
            request_json(base_url, '/api/datasets', {})
            return base_url.rstrip('/')
        except Exception as error:
            last_error = error
    raise SystemExit(f'No reachable web app base URL. Last error: {last_error}')


def verify_routes(base_url: str, dataset: str, source_id: str, window: VerificationWindow) -> None:
    datasets = request_json(base_url, '/api/datasets', {})
    summaries = dataset_summaries(datasets)
    if not any(summary.get('datasetId') == dataset for summary in summaries):
        raise SystemExit(f'/api/datasets did not include datasetId={dataset}')

    common = {
        'dataset': dataset,
        'routers': source_id,
        'startDate': str(window.start),
        'endDate': str(window.end),
    }
    assert_nonempty(
        request_json(base_url, '/api/netflow/stats', {**common, 'groupBy': 'hour'}),
        'result',
        '/api/netflow/stats',
    )
    for route in ('/api/ip/stats', '/api/protocol/stats'):
        assert_nonempty(
            request_json(base_url, route, {**common, 'granularity': '1h'}),
            'buckets',
            route,
        )
    for route in ('/api/netflow/structure-stats', '/api/netflow/spectrum-stats'):
        assert_nonempty(
            request_json(base_url, route, {**common, 'granularity': '5m'}),
            'buckets',
            route,
        )

    slug = bucket_slug(window.detail_bucket)
    assert_nonempty(
        request_json(base_url, f'/api/netflow/files/{slug}/details', {'dataset': dataset}),
        'routers',
        '/api/netflow/files/[slug]/details',
    )


def request_json(base_url: str, route: str, params: dict[str, str]) -> object:
    query = urllib.parse.urlencode(params)
    url = f'{base_url.rstrip("/")}{route}'
    if query:
        url = f'{url}?{query}'
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            return json.loads(response.read().decode('utf-8'))
    except urllib.error.HTTPError as error:
        body = error.read().decode('utf-8', errors='replace')
        raise RuntimeError(f'{url} returned {error.code}: {body}') from error


def dataset_summaries(payload: object) -> list[dict]:
    """Normalize the dataset API response shape used by apps/web."""
    if isinstance(payload, list):
        return [item for item in payload if isinstance(item, dict)]
    if isinstance(payload, dict):
        value = payload.get('data', payload.get('datasets', []))
        if isinstance(value, list):
            return [item for item in value if isinstance(item, dict)]
    return []


def assert_nonempty(payload: object, key: str, route: str) -> None:
    if not isinstance(payload, dict):
        raise SystemExit(f'{route} returned non-object JSON')
    if 'error' in payload:
        raise SystemExit(f'{route} returned error: {payload["error"]}')
    value = payload.get(key)
    if not isinstance(value, list) or not value:
        raise SystemExit(f'{route} returned empty {key}')


def bucket_slug(bucket_start: int) -> str:
    return datetime.fromtimestamp(bucket_start, PACIFIC).strftime('%Y%m%d%H%M')


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
