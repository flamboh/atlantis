import importlib
import json
import sqlite3
from pathlib import Path

import pytest


def load_modules():
    pipeline_v2 = importlib.import_module('pipeline_v2')
    verifier = importlib.import_module('verify_web_compatible_v2')
    return importlib.reload(pipeline_v2), importlib.reload(verifier)


def build_two_flow_csv_db(tmp_path: Path, pipeline_v2) -> Path:
    mapping_path = tmp_path / 'mapping.json'
    mapping_path.write_text(
        json.dumps(
            {
                'timestamp_format': 'unix',
                'columns': {
                    'time_received': 'received_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'protocol': 'pr',
                    'packets': 'pkt',
                    'bytes': 'byt',
                },
                'source_id': {'value': 'ugr16'},
            }
        ),
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '\n'.join(
            [
                'received_at,src,dst,pr,pkt,byt',
                '1744732801,192.0.2.1,198.51.100.1,6,10,1000',
                '1744733101,192.0.2.2,198.51.100.2,17,20,2000',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    db_path = tmp_path / 'netflow.sqlite'
    with sqlite3.connect(db_path) as conn:
        pipeline_v2.process_input_specs(
            conn,
            [
                {
                    'input_kind': 'csv',
                    'path': str(csv_path),
                    'mapping_path': str(mapping_path),
                }
            ],
            max_workers=1,
            run_maad=False,
        )
    return db_path


def test_verify_web_compatible_v2_accepts_pipeline_csv_db(tmp_path: Path) -> None:
    pipeline_v2, verifier = load_modules()
    db_path = build_two_flow_csv_db(tmp_path, pipeline_v2)

    verifier.verify_database(
        db_path,
        source_id='ugr16',
        require_data=True,
        require_processed=True,
        require_no_raw_ip=True,
    )


def test_verify_web_compatible_v2_requires_maad_rows_when_requested(tmp_path: Path) -> None:
    pipeline_v2, verifier = load_modules()
    db_path = build_two_flow_csv_db(tmp_path, pipeline_v2)

    with pytest.raises(SystemExit, match='structure_stats_v2 has no rows'):
        verifier.verify_database(
            db_path,
            source_id='ugr16',
            require_data=True,
            require_maad_data=True,
            require_processed=True,
        )


def test_verify_web_compatible_v2_requires_netflow_rollup_rows(tmp_path: Path) -> None:
    pipeline_v2, verifier = load_modules()
    db_path = build_two_flow_csv_db(tmp_path, pipeline_v2)
    with sqlite3.connect(db_path) as conn:
        conn.execute("DELETE FROM netflow_stats_v2 WHERE granularity != '5m'")

    with pytest.raises(SystemExit, match='netflow_stats_v2 has no rollup rows'):
        verifier.verify_database(
            db_path,
            source_id='ugr16',
            require_data=True,
        )


def test_verify_web_compatible_v2_rejects_netflow_rollup_mismatch(tmp_path: Path) -> None:
    pipeline_v2, verifier = load_modules()
    db_path = build_two_flow_csv_db(tmp_path, pipeline_v2)
    with sqlite3.connect(db_path) as conn:
        conn.execute(
            """
            UPDATE netflow_stats_v2
            SET flows = flows + 1
            WHERE granularity = '1h'
            """
        )

    with pytest.raises(SystemExit, match='netflow_stats_v2 rollup parity failed'):
        verifier.verify_database(
            db_path,
            source_id='ugr16',
            require_data=True,
            require_processed=True,
        )


def test_verify_web_compatible_v2_rejects_persisted_raw_ipv4_text(tmp_path: Path) -> None:
    pipeline_v2, verifier = load_modules()
    db_path = build_two_flow_csv_db(tmp_path, pipeline_v2)
    with sqlite3.connect(db_path) as conn:
        conn.execute(
            """
            UPDATE protocol_stats_v2
            SET protocols_list_ipv4 = ?
            WHERE source_id = ?
            """,
            ('192.0.2.55', 'ugr16'),
        )

    with pytest.raises(SystemExit, match='protocol_stats_v2.protocols_list_ipv4 contains raw IPv4 literal'):
        verifier.verify_database(
            db_path,
            source_id='ugr16',
            require_data=True,
            require_processed=True,
            require_no_raw_ip=True,
        )
