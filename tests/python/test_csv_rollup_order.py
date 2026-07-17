import importlib
import json
import sqlite3
from datetime import datetime, timezone

import pytest


def load_pipeline():
    return importlib.reload(importlib.import_module('pipeline'))


def write_mapping(path, *, source_column: bool = False) -> None:
    columns = {
        'time_end': 'time',
        'src_ip': 'src',
        'dst_ip': 'dst',
        'protocol': 'protocol',
        'packets': 'packets',
        'bytes': 'bytes',
        'src_tos': 'tos',
    }
    fieldnames = ['time', 'src', 'dst', 'protocol', 'packets', 'bytes', 'tos']
    source_id = {'value': 'r1'}
    if source_column:
        columns['source_id'] = 'source'
        fieldnames.append('source')
        source_id = {'column': 'source'}
    path.write_text(
        json.dumps(
            {
                'has_header': False,
                'input_order': 'timestamp_ascending',
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'UTC',
                'fieldnames': fieldnames,
                'columns': columns,
                'source_id': source_id,
            }
        ),
        encoding='utf-8',
    )


def csv_spec(path, mapping_path) -> dict:
    return {'input_kind': 'csv', 'path': str(path), 'mapping_path': str(mapping_path)}


def assert_rollups_match_five_minute_truth(conn: sqlite3.Connection) -> None:
    aggregate_rows = conn.execute(
        """
        SELECT source_id, granularity, bucket_start, bucket_end, flows
        FROM traffic_stats
        WHERE granularity != '5m'
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        ORDER BY source_id, granularity, bucket_start
        """
    ).fetchall()
    assert aggregate_rows
    for source_id, _granularity, bucket_start, bucket_end, flows in aggregate_rows:
        expected = conn.execute(
            """
            SELECT COALESCE(SUM(flows), 0)
            FROM traffic_stats
            WHERE source_id = ?
              AND granularity = '5m'
              AND bucket_start >= ?
              AND bucket_start < ?
              AND ip_version = 4
              AND src_visibility = 'all'
              AND dst_visibility = 'all'
            """,
            (source_id, bucket_start, bucket_end),
        ).fetchone()[0]
        assert flows == expected


@pytest.mark.parametrize('reverse_specs', [False, True])
def test_split_csv_rollups_do_not_depend_on_spec_order(tmp_path, reverse_specs: bool) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    older_path = tmp_path / 'a-older.csv'
    newer_path = tmp_path / 'z-newer.csv'
    write_mapping(mapping_path)
    older_path.write_text(
        '2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n'
        '2025-01-15 00:05:00,192.0.2.2,198.51.100.2,17,1,20,0\n',
        encoding='utf-8',
    )
    newer_path.write_text(
        '2025-01-15 00:10:00,192.0.2.3,198.51.100.3,6,1,30,0\n',
        encoding='utf-8',
    )
    specs = [csv_spec(older_path, mapping_path), csv_spec(newer_path, mapping_path)]
    if reverse_specs:
        specs.reverse()

    pipeline.process_input_specs(conn, specs, run_maad=False)

    assert conn.execute(
        """
        SELECT SUM(flows)
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '5m'
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """
    ).fetchone() == (3,)
    assert_rollups_match_five_minute_truth(conn)


def test_csv_rollups_keep_sources_independent(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    csv_path = tmp_path / 'sources.csv'
    write_mapping(mapping_path, source_column=True)
    csv_path.write_text(
        '2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10,0,r1\n'
        '2025-01-15 00:00:00,192.0.2.2,198.51.100.2,17,1,20,0,r2\n'
        '2025-01-15 00:05:00,192.0.2.3,198.51.100.3,6,1,30,0,r1\n'
        '2025-01-15 00:05:00,192.0.2.4,198.51.100.4,17,1,40,0,r2\n',
        encoding='utf-8',
    )

    pipeline.process_input_specs(conn, [csv_spec(csv_path, mapping_path)], run_maad=False)

    assert conn.execute(
        """
        SELECT source_id, SUM(flows)
        FROM traffic_stats
        WHERE granularity = '5m'
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        GROUP BY source_id
        ORDER BY source_id
        """
    ).fetchall() == [('r1', 2), ('r2', 2)]
    assert_rollups_match_five_minute_truth(conn)


def test_overlapping_csv_bucket_is_rejected_instead_of_double_counted(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    first_path = tmp_path / 'a.csv'
    second_path = tmp_path / 'b.csv'
    write_mapping(mapping_path)
    first_path.write_text(
        '2025-01-15 00:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    second_path.write_text(
        '2025-01-15 00:00:00,192.0.2.2,198.51.100.2,17,1,20,0\n',
        encoding='utf-8',
    )

    with pytest.raises(ValueError, match='Overlapping canonical 5m input'):
        pipeline.process_input_specs(
            conn,
            [csv_spec(second_path, mapping_path), csv_spec(first_path, mapping_path)],
            run_maad=False,
        )

    assert conn.execute('SELECT COUNT(*) FROM traffic_stats').fetchone() == (0,)
    assert conn.execute('SELECT COUNT(*) FROM processed_inputs').fetchone() == (0,)


def test_split_csv_that_moves_backwards_in_stable_path_order_fails_cleanly(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    newer_path = tmp_path / 'a-newer.csv'
    older_path = tmp_path / 'z-older.csv'
    write_mapping(mapping_path)
    newer_path.write_text(
        '2025-01-15 00:10:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    older_path.write_text(
        '2025-01-15 00:00:00,192.0.2.2,198.51.100.2,17,1,20,0\n',
        encoding='utf-8',
    )

    with pytest.raises(ValueError, match='moved backwards across input scans'):
        pipeline.process_input_specs(
            conn,
            [csv_spec(older_path, mapping_path), csv_spec(newer_path, mapping_path)],
            run_maad=False,
        )

    assert conn.execute('SELECT COUNT(*) FROM traffic_stats').fetchone() == (0,)
    assert conn.execute('SELECT COUNT(*) FROM processed_inputs').fetchone() == (0,)


def test_adjacent_csv_bucket_in_later_run_cannot_replace_partial_rollups(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    first_path = tmp_path / 'first.csv'
    second_path = tmp_path / 'second.csv'
    write_mapping(mapping_path)
    first_path.write_text(
        '2025-01-15 14:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    second_path.write_text(
        '2025-01-15 14:05:00,192.0.2.2,198.51.100.2,17,1,20,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, [csv_spec(first_path, mapping_path)], run_maad=False)
    before = conn.execute(
        """
        SELECT granularity, bucket_start, flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        ORDER BY granularity, bucket_start
        """
    ).fetchall()

    with pytest.raises(ValueError, match='Cannot reopen a persisted aggregate interval exactly'):
        pipeline.process_input_specs(conn, [csv_spec(second_path, mapping_path)], run_maad=False)

    assert conn.execute(
        """
        SELECT granularity, bucket_start, flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        ORDER BY granularity, bucket_start
        """
    ).fetchall() == before
    assert_rollups_match_five_minute_truth(conn)


def test_adjacent_nfcapd_bucket_cannot_reopen_csv_rollups(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    csv_path = tmp_path / 'flows.csv'
    write_mapping(mapping_path)
    csv_path.write_text(
        '2025-01-15 14:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, [csv_spec(csv_path, mapping_path)], run_maad=False)
    bucket_start = conn.execute(
        """
        SELECT bucket_start
        FROM traffic_stats
        WHERE source_id = 'r1' AND granularity = '5m'
        LIMIT 1
        """
    ).fetchone()[0]

    with pytest.raises(ValueError, match='Cannot reopen a persisted aggregate interval exactly'):
        pipeline.process_input_specs(
            conn,
            [
                {
                    'input_kind': 'nfcapd',
                    'path': '/captures/r1/nfcapd.202501151405',
                    'source_id': 'r1',
                    'bucket_start': bucket_start + 300,
                    'gap': True,
                }
            ],
            run_maad=False,
            max_workers=1,
        )

    assert conn.execute(
        """
        SELECT COUNT(*)
        FROM traffic_stats
        WHERE source_id = 'r1' AND granularity = '5m'
        """
    ).fetchone() == (10,)
    assert_rollups_match_five_minute_truth(conn)


def test_exact_csv_bucket_cannot_be_overwritten_by_nfcapd_gap(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    csv_path = tmp_path / 'flows.csv'
    write_mapping(mapping_path)
    csv_path.write_text(
        '2025-01-15 14:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, [csv_spec(csv_path, mapping_path)], run_maad=False)
    bucket_start = conn.execute(
        "SELECT MIN(bucket_start) FROM traffic_stats WHERE granularity = '5m'"
    ).fetchone()[0]

    with pytest.raises(ValueError, match='Overlapping canonical 5m input'):
        pipeline.process_input_specs(
            conn,
            [
                {
                    'input_kind': 'nfcapd',
                    'path': '/captures/r1/nfcapd.202501151400',
                    'source_id': 'r1',
                    'bucket_start': bucket_start,
                    'gap': True,
                }
            ],
            run_maad=False,
            max_workers=1,
        )

    assert conn.execute(
        """
        SELECT flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '5m'
          AND bucket_start = ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (bucket_start,),
    ).fetchone() == (1,)
    assert_rollups_match_five_minute_truth(conn)


def test_exact_nfcapd_bucket_cannot_be_overwritten_by_csv(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    bucket_start = int(datetime(2025, 1, 15, 14, tzinfo=timezone.utc).timestamp())
    pipeline.process_input_specs(
        conn,
        [
            {
                'input_kind': 'nfcapd',
                'path': '/captures/r1/nfcapd.202501151400',
                'source_id': 'r1',
                'bucket_start': bucket_start,
                'gap': True,
            }
        ],
        run_maad=False,
        max_workers=1,
    )
    mapping_path = tmp_path / 'mapping.json'
    csv_path = tmp_path / 'flows.csv'
    write_mapping(mapping_path)
    csv_path.write_text(
        '2025-01-15 14:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )

    with pytest.raises(ValueError, match='Overlapping canonical 5m input'):
        pipeline.process_input_specs(conn, [csv_spec(csv_path, mapping_path)], run_maad=False)

    assert conn.execute(
        """
        SELECT flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '5m'
          AND bucket_start = ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (bucket_start,),
    ).fetchone() == (0,)
    assert_rollups_match_five_minute_truth(conn)


def test_exact_csv_bucket_cannot_be_overwritten_by_logical_nfcapd_gap(tmp_path) -> None:
    pipeline = load_pipeline()
    conn = sqlite3.connect(':memory:')
    mapping_path = tmp_path / 'mapping.json'
    csv_path = tmp_path / 'flows.csv'
    write_mapping(mapping_path)
    csv_path.write_text(
        '2025-01-15 14:00:00,192.0.2.1,198.51.100.1,6,1,10,0\n',
        encoding='utf-8',
    )
    pipeline.process_input_specs(conn, [csv_spec(csv_path, mapping_path)], run_maad=False)
    bucket_start = conn.execute(
        "SELECT MIN(bucket_start) FROM traffic_stats WHERE granularity = '5m'"
    ).fetchone()[0]

    with pytest.raises(ValueError, match='Overlapping canonical 5m input'):
        pipeline.process_nfcapd_logical_bucket_jobs(
            conn,
            [
                {
                    'source_id': 'r1',
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_start + 300,
                    'member_specs': [],
                    'missing_members': ['r1'],
                }
            ],
            maad_bin='',
                maad_backend='subprocess',
            maad_workers=1,
            max_workers=1,
            run_maad=False,
        )

    assert conn.execute(
        """
        SELECT flows
        FROM traffic_stats
        WHERE source_id = 'r1'
          AND granularity = '5m'
          AND bucket_start = ?
          AND ip_version = 4
          AND src_visibility = 'all'
          AND dst_visibility = 'all'
        """,
        (bucket_start,),
    ).fetchone() == (1,)
    assert_rollups_match_five_minute_truth(conn)
