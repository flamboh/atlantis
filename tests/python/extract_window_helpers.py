import importlib
import sqlite3
import sys
from pathlib import Path

import datasets_metadata
import processed_inputs
import statistical_bucket
import stats
from input_revision import InputRevision


NETFLOW_DB_TOOLS = Path(__file__).resolve().parents[2] / 'tools' / 'netflow-db'
if str(NETFLOW_DB_TOOLS) not in sys.path:
    sys.path.insert(0, str(NETFLOW_DB_TOOLS))

SQLITE_SIDECAR_SUFFIXES = ('-wal', '-shm', '-journal')
PORTABLE_TABLES = {
    'traffic_stats',
    'protocol_stats',
    'address_count_stats',
    'port_count_stats',
    'address_structure_stats',
}


def load_module():
    module = importlib.import_module('extract_window.cli')
    return importlib.reload(module)


def make_source_db(path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    datasets_metadata.init_datasets_table(conn)
    processed_inputs.init_processed_inputs_table(conn)
    stats.init_stats_tables(conn)
    datasets_metadata.upsert_dataset_metadata(
        conn,
        {
            'dataset_id': 'uoregon',
            'label': 'UONet',
            'default_start_date': '2025-02-01',
            'source_mode': 'static',
            'discovery_mode': 'static',
            'sort_order': 7,
            'sources': [
                {'source_id': 'r1', 'members': ['r1-a', 'r1-b']},
                {'source_id': 'r2', 'members': ['r2-a']},
            ],
        },
    )
    input_revision = InputRevision.create(
        input_kind='nfcapd',
        locator='nfcapd://r1/197001010000',
        content_fingerprint='fixture',
        decoder_fingerprint='fixture',
    )
    processed_inputs.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator='nfcapd://r1/197001010000',
        source_id='r1',
        bucket_start=200,
        bucket_end=500,
        input_revision=input_revision,
    )
    stats.insert_traffic_stats_rows(
        conn,
        [
            traffic_row(source_id='r1', granularity='5m', bucket_start=100, flows=10),
            traffic_row(source_id='r1', granularity='5m', bucket_start=200, flows=20),
            traffic_row(source_id='r2', granularity='5m', bucket_start=200, flows=30),
            traffic_row(source_id='r1', granularity='1h', bucket_start=200, flows=40),
            traffic_row(source_id='r1', granularity='5m', bucket_start=500, flows=50),
        ],
    )
    conn.commit()
    return conn


def traffic_row(
    *,
    source_id: str,
    granularity: str,
    bucket_start: int,
    flows: int,
) -> dict:
    bucket = statistical_bucket.CanonicalBucket(
        key=statistical_bucket.BucketKey(
            source_id,
            granularity,
            bucket_start,
            bucket_start + 300,
        ),
        traffic=(
            statistical_bucket.ScopedTraffic(
                statistical_bucket.Scope(4, 'all', 'all'),
                statistical_bucket.TrafficMetrics(
                    flows=flows,
                    packets=flows * 10,
                    bytes=flows * 100,
                ),
            ),
        ),
        protocols=(),
        addresses=(),
        five_minute_starts=frozenset({bucket_start}),
    )
    return stats.canonical_bucket_rows(bucket)['traffic_rows'][0]


def table_sql(conn: sqlite3.Connection, name: str) -> str:
    row = conn.execute(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
        (name,),
    ).fetchone()
    assert row is not None
    return str(row['sql'])


def index_names(conn: sqlite3.Connection, table: str) -> set[str]:
    return {
        str(row['name'])
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ?",
            (table,),
        )
    }


def copied_rows(conn: sqlite3.Connection) -> list[tuple]:
    rows = conn.execute(
        """
        SELECT source_id, granularity, bucket_start, flows
        FROM traffic_stats
        ORDER BY source_id, granularity, bucket_start
        """
    ).fetchall()
    return [tuple(row) for row in rows]


def sqlite_table_names(conn: sqlite3.Connection) -> set[str]:
    return {
        str(row['name'])
        for row in conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
        )
    }


def sqlite_sidecar_paths(sqlite_path: Path) -> list[Path]:
    return [sqlite_path.with_name(sqlite_path.name + suffix) for suffix in SQLITE_SIDECAR_SUFFIXES]


def write_stale_sqlite_sidecars(sqlite_path: Path, generation: str) -> dict[Path, bytes]:
    sidecars = {}
    for suffix, sidecar_path in zip(
        SQLITE_SIDECAR_SUFFIXES,
        sqlite_sidecar_paths(sqlite_path),
        strict=True,
    ):
        sidecar_bytes = f'{generation}:{sqlite_path.name}{suffix}'.encode()
        sidecar_path.write_bytes(sidecar_bytes)
        sidecars[sidecar_path] = sidecar_bytes
    return sidecars


def extract_args(
    module,
    source_db: Path,
    output_dir: Path,
    *extra_args: str,
    start: str = '150',
    end: str = '500',
):
    args = ['--source-db', str(source_db), '--output-dir', str(output_dir)]
    args += ['--start', start, '--end', end, *extra_args]
    return module.parse_args(args)
