import importlib
import json
import sqlite3
import tarfile
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest


def load_modules():
    csv_ingest_v2 = importlib.import_module('csv_ingest_v2')
    csv_inputs_v2 = importlib.import_module('csv_inputs_v2')
    pipeline_v2 = importlib.import_module('pipeline_v2')
    normalized_rows_v2 = importlib.import_module('normalized_rows_v2')
    importlib.reload(csv_ingest_v2)
    importlib.reload(normalized_rows_v2)
    importlib.reload(csv_inputs_v2)
    return importlib.reload(pipeline_v2), normalized_rows_v2


def test_process_input_specs_populates_v2_tables_for_csv(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
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
                'source_id': {'value': 'uo-feed'},
            }
        ),
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '\n'.join(
            [
                'received_at,src,dst,pr,pkt,byt',
                '1744733279,192.0.2.1,198.51.100.9,6,10,1000',
                '1744733001,2001:db8::1,2001:db8::2,58,3,300',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    conn = sqlite3.connect(':memory:')

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
    )

    processed_inputs = conn.execute(
        'SELECT input_kind, input_locator, source_id, bucket_start, status FROM processed_inputs_v2 ORDER BY bucket_start'
    ).fetchall()
    netflow = conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, packets, bytes FROM netflow_stats_v2 ORDER BY bucket_start, ip_version'
    ).fetchall()
    ip_stats = conn.execute(
        "SELECT source_id, bucket_start, sa_ipv4_count, sa_ipv6_count FROM ip_stats_v2 WHERE granularity = '5m' ORDER BY bucket_start"
    ).fetchall()
    protocol_stats = conn.execute(
        "SELECT source_id, bucket_start, protocols_list_ipv4, protocols_list_ipv6 FROM protocol_stats_v2 WHERE granularity = '5m' ORDER BY bucket_start"
    ).fetchall()

    assert processed_inputs == [
        ('csv', str(csv_path), 'uo-feed', 1744732800, 'processed'),
        ('csv', str(csv_path), 'uo-feed', 1744733100, 'processed'),
    ]
    assert netflow == [
        ('uo-feed', 1744732800, 6, 1, 3, 300),
        ('uo-feed', 1744733100, 4, 1, 10, 1000),
    ]
    assert ip_stats == [
        ('uo-feed', 1744732800, 0, 1),
        ('uo-feed', 1744733100, 1, 0),
    ]
    assert protocol_stats == [
        ('uo-feed', 1744732800, '', '58'),
        ('uo-feed', 1744733100, '6', ''),
    ]


def test_process_input_specs_rejects_header_csv_missing_mapped_column(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = tmp_path / 'mapping.json'
    mapping_path.write_text(
        json.dumps(
            {
                'timestamp_format': 'unix',
                'columns': {
                    'time_received': 'received_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                    'packets': 'pkt',
                },
                'source_id': {'value': 'uo-feed'},
            }
        ),
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        'received_at,src,dst\n1744733279,192.0.2.1,198.51.100.9\n',
        encoding='utf-8',
    )

    with pytest.raises(pipeline_v2.CsvSourceConfigError, match='pkt'):
        pipeline_v2.process_input_specs(
            sqlite3.connect(':memory:'),
            [
                {
                    'input_kind': 'csv',
                    'path': str(csv_path),
                    'mapping_path': str(mapping_path),
                }
            ],
            max_workers=1,
        )


def test_process_input_specs_rejects_late_csv_rows_after_streaming_cutoff(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = tmp_path / 'mapping.json'
    mapping_path.write_text(
        json.dumps(
            {
                'timestamp_format': 'unix',
                'out_of_order_lag_buckets': 0,
                'columns': {
                    'time_received': 'received_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'value': 'uo-feed'},
            }
        ),
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '\n'.join(
            [
                'received_at,src,dst',
                '1744733401,192.0.2.1,198.51.100.1',
                '1744732801,192.0.2.2,198.51.100.2',
            ]
        )
        + '\n',
        encoding='utf-8',
    )

    with pytest.raises(ValueError, match='not ordered enough'):
        pipeline_v2.process_input_specs(
            sqlite3.connect(':memory:'),
            [
                {
                    'input_kind': 'csv',
                    'path': str(csv_path),
                    'mapping_path': str(mapping_path),
                }
            ],
            max_workers=1,
        )


def test_process_input_specs_uses_nfdump_adapter_for_nfcapd(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')

    def fake_build_nfcapd_bucket_payload(path: str, source_id: str):
        assert path == '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005'
        assert source_id == 'oh_ir1_gw'
        return {
            'processed_bucket': {
                'input_kind': 'nfcapd',
                'input_locator': path,
                'source_id': source_id,
                'bucket_start': 1744733100,
                'bucket_end': 1744733400,
            },
            'netflow_rows': [
                {
                    'source_id': 'oh_ir1_gw',
                    'bucket_start': 1744733100,
                    'bucket_end': 1744733400,
                    'ip_version': 4,
                    'flows': 1,
                    'flows_tcp': 1,
                    'flows_udp': 0,
                    'flows_icmp': 0,
                    'flows_other': 0,
                    'packets': 10,
                    'packets_tcp': 10,
                    'packets_udp': 0,
                    'packets_icmp': 0,
                    'packets_other': 0,
                    'bytes': 1000,
                    'bytes_tcp': 1000,
                    'bytes_udp': 0,
                    'bytes_icmp': 0,
                    'bytes_other': 0,
                },
                {
                    'source_id': 'oh_ir1_gw',
                    'bucket_start': 1744733100,
                    'bucket_end': 1744733400,
                    'ip_version': 6,
                    'flows': 1,
                    'flows_tcp': 0,
                    'flows_udp': 0,
                    'flows_icmp': 1,
                    'flows_other': 0,
                    'packets': 3,
                    'packets_tcp': 0,
                    'packets_udp': 0,
                    'packets_icmp': 3,
                    'packets_other': 0,
                    'bytes': 300,
                    'bytes_tcp': 0,
                    'bytes_udp': 0,
                    'bytes_icmp': 300,
                    'bytes_other': 0,
                },
            ],
            'ip_row': {
                'source_id': 'oh_ir1_gw',
                'granularity': '5m',
                'bucket_start': 1744733100,
                'bucket_end': 1744733400,
                'sa_ipv4_count': 1,
                'da_ipv4_count': 1,
                'sa_ipv6_count': 1,
                'da_ipv6_count': 1,
            },
            'protocol_row': {
                'source_id': 'oh_ir1_gw',
                'granularity': '5m',
                'bucket_start': 1744733100,
                'bucket_end': 1744733400,
                'unique_protocols_count_ipv4': 1,
                'unique_protocols_count_ipv6': 1,
                'protocols_list_ipv4': '6',
                'protocols_list_ipv6': '58',
            },
            'raw_bucket': {
                'source_id': 'oh_ir1_gw',
                'bucket_start': 1744733100,
                'source_ipv4': ['192.0.2.1'],
                'destination_ipv4': ['198.51.100.9'],
                'source_ipv6': ['2001:db8::1'],
                'destination_ipv6': ['2001:db8::2'],
                'protocols_ipv4': ['6'],
                'protocols_ipv6': ['58'],
                'maad_source_ipv4': ['192.0.2.1'],
                'maad_destination_ipv4': ['198.51.100.9'],
                'netflow_rows': [
                    {
                        'source_id': 'oh_ir1_gw',
                        'bucket_start': 1744733100,
                        'bucket_end': 1744733400,
                        'ip_version': 4,
                        'flows': 1,
                        'flows_tcp': 1,
                        'flows_udp': 0,
                        'flows_icmp': 0,
                        'flows_other': 0,
                        'packets': 10,
                        'packets_tcp': 10,
                        'packets_udp': 0,
                        'packets_icmp': 0,
                        'packets_other': 0,
                        'bytes': 1000,
                        'bytes_tcp': 1000,
                        'bytes_udp': 0,
                        'bytes_icmp': 0,
                        'bytes_other': 0,
                    },
                    {
                        'source_id': 'oh_ir1_gw',
                        'bucket_start': 1744733100,
                        'bucket_end': 1744733400,
                        'ip_version': 6,
                        'flows': 1,
                        'flows_tcp': 0,
                        'flows_udp': 0,
                        'flows_icmp': 1,
                        'flows_other': 0,
                        'packets': 3,
                        'packets_tcp': 0,
                        'packets_udp': 0,
                        'packets_icmp': 3,
                        'packets_other': 0,
                        'bytes': 300,
                        'bytes_tcp': 0,
                        'bytes_udp': 0,
                        'bytes_icmp': 300,
                        'bytes_other': 0,
                    },
                ],
            },
        }

    monkeypatch.setattr(pipeline_v2, 'build_nfcapd_bucket_payload', fake_build_nfcapd_bucket_payload)
    monkeypatch.setattr(
        pipeline_v2,
        'write_aggregate_rows',
        lambda *args, **kwargs: pytest.fail('nfcapd inputs should use streaming aggregate state'),
    )

    pipeline_v2.process_input_specs(
        conn,
        [
            {
                'input_kind': 'nfcapd',
                'path': '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
                'source_id': 'oh_ir1_gw',
            }
        ],
        max_workers=1,
    )

    netflow = conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows FROM netflow_stats_v2 ORDER BY ip_version'
    ).fetchall()
    processed_inputs = conn.execute(
        'SELECT input_kind, input_locator, source_id, bucket_start, status FROM processed_inputs_v2'
    ).fetchall()
    aggregate_netflow = conn.execute(
        """
        SELECT granularity, ip_version, flows, packets, bytes
        FROM netflow_stats_aggregate_v2
        ORDER BY granularity, ip_version
        """
    ).fetchall()

    assert netflow == [
        ('oh_ir1_gw', 1744733100, 4, 1),
        ('oh_ir1_gw', 1744733100, 6, 1),
    ]
    assert processed_inputs == [
        (
            'nfcapd',
            '/captures/oh_ir1_gw/2025/04/15/nfcapd.202504150005',
            'oh_ir1_gw',
            1744733100,
            'processed',
        )
    ]
    assert aggregate_netflow == [
        ('1d', 4, 1, 10, 1000),
        ('1d', 6, 1, 3, 300),
        ('1h', 4, 1, 10, 1000),
        ('1h', 6, 1, 3, 300),
        ('30m', 4, 1, 10, 1000),
        ('30m', 6, 1, 3, 300),
    ]


def write_ugr16_mapping(path: Path) -> Path:
    path.write_text(
        json.dumps(
            {
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'Europe/Madrid',
                'has_header': False,
                'fieldnames': [
                    'time_end',
                    'duration',
                    'src_ip',
                    'dst_ip',
                    'src_port',
                    'dst_port',
                    'protocol',
                    'flags',
                    'forwarding_status',
                    'src_tos',
                    'packets',
                    'bytes',
                    'label',
                ],
                'skip_bad_column_count': True,
                'archive': {'member_contains': 'csv'},
                'discovery': {
                    'include_contains': [],
                    'include_suffixes': ['.tar.gz', '.tgz'],
                    'exclude_suffixes': ['.aria2', '.txt'],
                },
                'columns': {
                    'time_end': 'time_end',
                    'src_ip': 'src_ip',
                    'dst_ip': 'dst_ip',
                    'src_port': 'src_port',
                    'dst_port': 'dst_port',
                    'protocol': 'protocol',
                    'packets': 'packets',
                    'bytes': 'bytes',
                    'src_tos': 'src_tos',
                },
                'source_id': {'value': 'ugr16-test'},
            }
        ),
        encoding='utf-8',
    )
    return path


def test_process_input_specs_populates_v2_tables_for_ugr16_csv_config(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = write_ugr16_mapping(tmp_path / 'ugr16.mapping.json')
    csv_path = tmp_path / 'july.week5.csv'
    csv_path.write_text(
        '\n'.join(
            [
                '2016-08-001.169.173.160,80,45736,TCP,.AP.SF,0,0,5,948,background',
                '2016-07-27 13:43:00,0.000,42.219.154.106,143.72.8.136,59212,53,UDP,.A....,0,0,TCP,72,background',
                '2016-07-27 13:43:01,0.000,42.219.154.106,143.72.8.136,59212,53,Trnk1,.A....,0,0,1,72,background',
                '2016-07-27 13:43:30,0.000,42.219.154.107,143.72.8.137,59212,53,UDP,.A....,0,0,1,72,background',
                '2016-07-27 13:44:01,0.000,42.219.154.108,143.72.8.137,443,58676,TCP,.AP.S.,0,0,6,5298,background',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    conn = sqlite3.connect(':memory:')

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
    )

    netflow = conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, flows_tcp, flows_udp, packets, bytes FROM netflow_stats_v2'
    ).fetchall()
    processed_inputs = conn.execute(
        'SELECT input_kind, input_locator, source_id, bucket_start, status FROM processed_inputs_v2'
    ).fetchall()

    assert netflow == [
        ('ugr16-test', 1469619600, 4, 2, 1, 1, 7, 5370),
    ]
    assert processed_inputs == [
        ('csv', str(csv_path), 'ugr16-test', 1469619600, 'processed'),
    ]


def test_process_input_specs_uses_arrow_fast_path_for_ugr16_without_maad(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = write_ugr16_mapping(tmp_path / 'ugr16.mapping.json')
    csv_path = tmp_path / 'july.week5.csv'
    csv_path.write_text(
        '\n'.join(
            [
                '2016-08-001.169.173.160,80,45736,TCP,.AP.SF,0,0,5,948,background',
                '2016-07-27 13:43:30,0.000,42.219.154.107,143.72.8.137,59212,53,UDP,.A....,0,0,1,72,background',
                '2016-07-27 13:44:01,0.000,42.219.154.108,143.72.8.137,443,58676,TCP,.AP.S.,0,0,6,5298,background',
                '2016-07-27 13:44:30,0.000,999.219.154.109,143.72.8.139,443,58676,TCP,.AP.S.,0,0,6,5298,background',
                '2016-07-27 13:44:45,0.000,42.219.154.110,not-ip,443,58676,TCP,.AP.S.,0,0,6,5298,background',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    conn = sqlite3.connect(':memory:')

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

    netflow = conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, flows_tcp, flows_udp, packets, bytes FROM netflow_stats_v2'
    ).fetchall()
    ip_stats = conn.execute(
        "SELECT granularity, bucket_start, sa_ipv4_count, da_ipv4_count FROM ip_stats_v2 ORDER BY granularity, bucket_start"
    ).fetchall()
    processed_inputs = conn.execute(
        'SELECT input_kind, input_locator, source_id, bucket_start, status FROM processed_inputs_v2'
    ).fetchall()

    assert netflow == [
        ('ugr16-test', 1469619600, 4, 2, 1, 1, 7, 5370),
    ]
    assert ip_stats == [
        ('1d', 1469602800, 2, 1),
        ('1h', 1469617200, 2, 1),
        ('30m', 1469619000, 2, 1),
        ('5m', 1469619600, 2, 1),
    ]
    assert processed_inputs == [
        ('csv', str(csv_path), 'ugr16-test', 1469619600, 'processed'),
    ]


def test_process_input_specs_uses_arrow_fast_path_for_ugr16_with_streaming_maad(
    monkeypatch, tmp_path: Path
) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = write_ugr16_mapping(tmp_path / 'ugr16.mapping.json')
    csv_path = tmp_path / 'july.week5.csv'
    csv_path.write_text(
        '\n'.join(
            [
                '2016-07-27 13:43:30,0.000,42.219.154.107,143.72.8.137,59212,53,UDP,.A....,0,0,1,72,background',
                '2016-07-27 13:44:01,0.000,42.219.154.108,143.72.8.138,443,58676,TCP,.AP.S.,0,0,6,5298,background',
                '2016-07-27 13:44:30,0.000,999.219.154.109,143.72.8.139,443,58676,TCP,.AP.S.,0,0,6,5298,background',
                '2016-07-27 13:44:45,0.000,42.219.154.110,not-ip,443,58676,TCP,.AP.S.,0,0,6,5298,background',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    conn = sqlite3.connect(':memory:')
    maad_calls = []

    def fake_run_maad_json(maad_bin, addresses, *, timeout_seconds):
        maad_calls.append((str(maad_bin), sorted(addresses), timeout_seconds))
        return pipeline_v2.MaadJsonResult(
            schema_version=1,
            metadata={
                'input': '-',
                'minPrefixLength': 7,
                'maxPrefixLength': 23,
                'totalAddrs': len(addresses),
            },
            structure=[],
            spectrum=[],
            dimensions=[],
        )

    monkeypatch.setattr(pipeline_v2, 'run_maad_json', fake_run_maad_json)

    pipeline_v2.process_input_specs(
        conn,
        [
            {
                'input_kind': 'csv',
                'path': str(csv_path),
                'mapping_path': str(mapping_path),
            }
        ],
        maad_bin='/tmp/MAAD',
        max_workers=1,
    )

    maad_totals = conn.execute(
        """
        SELECT granularity,
               json_extract(metadata_json_sa, '$.totalAddrs'),
               json_extract(metadata_json_da, '$.totalAddrs')
        FROM structure_stats_v2
        ORDER BY granularity
        """
    ).fetchall()
    disk_objects = sorted(path.name for path in tmp_path.iterdir())

    assert maad_totals == [
        ('1d', 2, 2),
        ('1h', 2, 2),
        ('30m', 2, 2),
        ('5m', 2, 2),
    ]
    assert sorted(call[2] for call in maad_calls) == [300, 300, 600, 600, 900, 900, 1800, 1800]
    assert disk_objects == ['july.week5.csv', 'ugr16.mapping.json']


def test_process_input_specs_uses_arrow_fast_path_for_ugr16_tar_without_maad(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = write_ugr16_mapping(tmp_path / 'ugr16.mapping.json')
    member_path = tmp_path / 'july.week5.csv.uniqblacklistremoved'
    member_path.write_text(
        '\n'.join(
            [
                '2016-07-27 13:43:30,0.000,42.219.154.107,143.72.8.137,59212,53,UDP,.A....,0,0,1,72,background',
                '2016-07-27 13:44:01,0.000,42.219.154.108,143.72.8.137,443,58676,TCP,.AP.S.,0,0,6,5298,background',
            ]
        )
        + '\n',
        encoding='utf-8',
    )
    archive_path = tmp_path / 'july_week5_csv.tar.gz'
    with tarfile.open(archive_path, 'w:gz') as archive:
        archive.add(member_path, arcname='uniq/july.week5.csv.uniqblacklistremoved')
    conn = sqlite3.connect(':memory:')

    pipeline_v2.process_input_specs(
        conn,
        [
            {
                'input_kind': 'csv',
                'path': str(archive_path),
                'mapping_path': str(mapping_path),
            }
        ],
        max_workers=1,
        run_maad=False,
    )

    assert conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, flows_tcp, flows_udp, packets, bytes FROM netflow_stats_v2'
    ).fetchall() == [
        ('ugr16-test', 1469619600, 4, 2, 1, 1, 7, 5370),
    ]
    assert conn.execute('SELECT status FROM processed_inputs_v2').fetchall() == [('processed',)]


def test_csv_tree_discovers_ugr16_archives_and_extracted_files(monkeypatch, tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    mapping_path = write_ugr16_mapping(tmp_path / 'ugr16.mapping.json')
    root = tmp_path / 'ugr_csv'
    root.mkdir()
    archive_path = root / 'april_week2_csv.tar.gz'
    incomplete_archive_path = root / 'august_week3_csv.tar.gz'
    extracted_path = root / 'august.week1.csv'
    ignored_path = root / 'ugr16-csv-urls.txt'
    extracted_path.write_text('', encoding='utf-8')
    incomplete_archive_path.write_text('', encoding='utf-8')
    incomplete_archive_path.with_name(f'{incomplete_archive_path.name}.aria2').write_text(
        '',
        encoding='utf-8',
    )
    ignored_path.write_text('', encoding='utf-8')
    member_path = tmp_path / 'april.week2.csv.uniqblacklistremoved'
    member_path.write_text(
        '2016-04-04 00:00:00,0.000,42.219.154.107,143.72.8.137,59212,53,UDP,.A....,0,0,1,72,background\n',
        encoding='utf-8',
    )
    with tarfile.open(archive_path, 'w:gz') as archive:
        archive.add(member_path, arcname='uniq/april.week2.csv.uniqblacklistremoved')
    calls = []

    def fake_process_input_specs(conn, input_specs, *, maad_bin, max_workers, **kwargs):
        calls.append(input_specs)

    monkeypatch.setattr(pipeline_v2, 'process_input_specs', fake_process_input_specs)

    pipeline_v2.process_pipeline_v2_config(
        sqlite3.connect(':memory:'),
        {
            'inputs': [
                {
                    'input_kind': 'csv_tree',
                    'root_path': str(root),
                    'mapping_path': str(mapping_path),
                }
            ],
        },
    )

    assert calls == [
        [
            {'input_kind': 'csv', 'path': str(archive_path), 'mapping_path': str(mapping_path)},
        ]
    ]


def test_process_pipeline_v2_config_writes_dataset_metadata() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')

    pipeline_v2.process_pipeline_v2_config(
        conn,
        {
            'datasets': [
                {
                    'dataset_id': 'alpha',
                    'label': 'Alpha Dataset',
                    'default_start_date': '2025-03-01',
                    'source_mode': 'subdirs',
                    'discovery_mode': 'live',
                    'sort_order': 3,
                    'sources': [
                        {'source_id': 'router-a', 'members': ['router-a']},
                        {'source_id': 'all', 'members': ['router-a', 'router-b']},
                    ],
                }
            ],
            'inputs': [],
        },
    )

    assert conn.execute(
        """
        SELECT id, label, default_start_date, source_mode, discovery_mode, sort_order
        FROM datasets
        """
    ).fetchall() == [('alpha', 'Alpha Dataset', '2025-03-01', 'subdirs', 'live', 3)]
    assert conn.execute(
        """
        SELECT dataset_id, source_id, member_id
        FROM source_members
        ORDER BY source_id, member_id
        """
    ).fetchall() == [
        ('alpha', 'all', 'router-a'),
        ('alpha', 'all', 'router-b'),
        ('alpha', 'router-a', 'router-a'),
    ]


def test_build_dataset_tree_config_includes_dataset_metadata(monkeypatch, tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    common = importlib.import_module('common')
    dataset = {
        'dataset_id': 'alpha',
        'label': 'Alpha Dataset',
        'root_path': str(tmp_path / 'raw'),
        'db_path': str(tmp_path / 'netflow.sqlite'),
        'default_start_date': '2025-03-01',
        'source_mode': 'subdirs',
        'discovery_mode': 'live',
        'source_ids': ['router-a'],
    }
    monkeypatch.setattr(common, 'get_dataset_config', lambda dataset_id: dataset)
    monkeypatch.setattr(common, 'list_dataset_sources', lambda dataset_id: ['router-a'])

    config = pipeline_v2.build_dataset_tree_config(
        dataset_id='alpha',
        start_date='2025-03-01',
        database_path=tmp_path / 'out.sqlite',
    )

    assert config['datasets'] == [dataset]


def test_discover_nfcapd_tree_specs_uses_canonical_layout(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    root = tmp_path / 'uoregon'
    valid = [
        root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000',
        root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010005',
        root / 'oh_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000',
    ]
    ignored = root / 'oh_ir1_gw' / '2025' / '02' / '02' / 'nfcapd.202502020000'
    tmp_file = root / 'oh_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010005.tmp'
    for path in [*valid, ignored, tmp_file]:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('', encoding='utf-8')

    specs = pipeline_v2.discover_nfcapd_tree_specs(
        root_path=root,
        source_ids=['oh_ir1_gw', 'cc_ir1_gw'],
        day=datetime(2025, 2, 1),
    )

    assert specs == [
        {
            'input_kind': 'nfcapd',
            'path': str(valid[0]),
            'source_id': 'cc_ir1_gw',
        },
        {
            'input_kind': 'nfcapd',
            'path': str(valid[1]),
            'source_id': 'cc_ir1_gw',
        },
        {
            'input_kind': 'nfcapd',
            'path': str(valid[2]),
            'source_id': 'oh_ir1_gw',
        },
    ]


def test_discover_nfcapd_tree_specs_zero_fills_bounded_missing_buckets(tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    root = tmp_path / 'uoregon'
    first = root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000'
    second = root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010010'
    for path in [first, second]:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('', encoding='utf-8')

    specs = pipeline_v2.discover_nfcapd_tree_specs(
        root_path=root,
        source_ids=['cc_ir1_gw'],
        day=datetime(2025, 2, 1),
        source_bounds=pipeline_v2.discover_nfcapd_source_bounds(root, ['cc_ir1_gw']),
    )

    assert specs == [
        {
            'input_kind': 'nfcapd',
            'path': str(first),
            'source_id': 'cc_ir1_gw',
        },
        {
            'input_kind': 'nfcapd',
            'path': 'gap://nfcapd/cc_ir1_gw/202502010005',
            'expected_path': str(
                root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010005'
            ),
            'source_id': 'cc_ir1_gw',
            'bucket_start': pipeline_v2.parse_nfcapd_bucket_start(str(first)) + 300,
            'gap': True,
        },
        {
            'input_kind': 'nfcapd',
            'path': str(second),
            'source_id': 'cc_ir1_gw',
        },
    ]


def test_process_pipeline_v2_config_chunks_nfcapd_tree_by_day(monkeypatch, tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'uoregon'
    calls = []

    def fake_process_jobs(conn, jobs, *, maad_bin, max_workers, **kwargs):
        calls.append((jobs, str(maad_bin), max_workers))

    for day in ['01', '02']:
        for source_id in ['cc_ir1_gw', 'oh_ir1_gw']:
            path = root / source_id / '2025' / '02' / day / f'nfcapd.202502{day}0000'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('', encoding='utf-8')

    monkeypatch.setattr(pipeline_v2, 'process_nfcapd_logical_bucket_jobs', fake_process_jobs)

    pipeline_v2.process_pipeline_v2_config(
        conn,
        {
            'maad_bin': '/tmp/MAAD',
            'max_workers': 16,
            'inputs': [
                {
                    'input_kind': 'nfcapd_tree',
                    'root_path': str(root),
                    'source_ids': ['cc_ir1_gw', 'oh_ir1_gw'],
                    'start_date': '2025-02-01',
                    'end_date': '2025-02-02',
                    'zero_fill_gaps': False,
                }
            ],
        },
    )

    assert [len(jobs) for jobs, _, _ in calls] == [2, 2]
    assert calls[0][0][0]['member_specs'][0]['path'].endswith('/cc_ir1_gw/2025/02/01/nfcapd.202502010000')
    assert calls[1][0][1]['member_specs'][0]['path'].endswith('/oh_ir1_gw/2025/02/02/nfcapd.202502020000')
    assert all(maad_bin == '/tmp/MAAD' and max_workers == 16 for _, maad_bin, max_workers in calls)


def test_process_pipeline_v2_config_defaults_tree_end_date_to_latest_file(
    monkeypatch, tmp_path: Path
) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'uoregon'
    calls = []

    def fake_process_jobs(conn, jobs, *, maad_bin, max_workers, **kwargs):
        calls.append(jobs)

    for day in ['01', '03']:
        path = root / 'cc_ir1_gw' / '2025' / '02' / day / f'nfcapd.202502{day}0000'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('', encoding='utf-8')

    monkeypatch.setattr(pipeline_v2, 'process_nfcapd_logical_bucket_jobs', fake_process_jobs)

    pipeline_v2.process_pipeline_v2_config(
        conn,
        {
            'inputs': [
                {
                    'input_kind': 'nfcapd_tree',
                    'root_path': str(root),
                    'source_ids': ['cc_ir1_gw'],
                    'start_date': '2025-02-01',
                    'zero_fill_gaps': False,
                }
            ],
        },
    )

    assert [jobs[0]['member_specs'][0]['path'] for jobs in calls] == [
        str(root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000'),
        str(root / 'cc_ir1_gw' / '2025' / '02' / '03' / 'nfcapd.202502030000'),
    ]


def test_process_pipeline_v2_config_skips_fully_processed_tree_day(
    monkeypatch, tmp_path: Path
) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'uoregon'
    path = root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('', encoding='utf-8')
    pipeline_v2.init_processed_inputs_v2_table(conn)
    pipeline_v2.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator=str(path),
        source_id='cc_ir1_gw',
        bucket_start=1738396800,
        bucket_end=1738397100,
    )
    pipeline_v2.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator=str(path),
        source_id='cc_ir1_gw',
        bucket_start=1738396800,
        status='processed',
    )

    monkeypatch.setattr(
        pipeline_v2,
        'process_nfcapd_logical_bucket_jobs',
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError('processed day should skip')),
    )

    pipeline_v2.process_pipeline_v2_config(
        conn,
        {
            'inputs': [
                {
                    'input_kind': 'nfcapd_tree',
                    'root_path': str(root),
                    'source_ids': ['cc_ir1_gw'],
                    'start_date': '2025-02-01',
                    'end_date': '2025-02-01',
                }
            ],
        },
    )


def test_process_pipeline_v2_config_builds_union_source_jobs(monkeypatch, tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'uoregon'
    cc_path = root / 'cc_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000'
    oh_path = root / 'oh_ir1_gw' / '2025' / '02' / '01' / 'nfcapd.202502010000'
    for path in [cc_path, oh_path]:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('', encoding='utf-8')
    calls = []

    def fake_process_jobs(conn, jobs, *, maad_bin, max_workers, **kwargs):
        calls.append(jobs)

    monkeypatch.setattr(pipeline_v2, 'process_nfcapd_logical_bucket_jobs', fake_process_jobs)

    pipeline_v2.process_pipeline_v2_config(
        conn,
        {
            'inputs': [
                {
                    'input_kind': 'nfcapd_tree',
                    'root_path': str(root),
                    'sources': [
                        {'source_id': 'cc_ir1_gw', 'members': ['cc_ir1_gw']},
                        {'source_id': 'uoregon_all', 'members': ['cc_ir1_gw', 'oh_ir1_gw']},
                    ],
                    'start_date': '2025-02-01',
                    'end_date': '2025-02-01',
                    'zero_fill_gaps': False,
                }
            ],
        },
    )

    assert [(job['source_id'], len(job['member_specs'])) for job in calls[0]] == [
        ('cc_ir1_gw', 1),
        ('uoregon_all', 2),
    ]


def test_build_nfcapd_logical_bucket_jobs_keeps_empty_union_gap() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    bucket_start = 1738396800
    member_specs = [
        {
            'input_kind': 'nfcapd',
            'path': 'gap://nfcapd/cc_ir1_gw/202502010000',
            'source_id': 'cc_ir1_gw',
            'bucket_start': bucket_start,
            'gap': True,
        },
        {
            'input_kind': 'nfcapd',
            'path': 'gap://nfcapd/oh_ir1_gw/202502010000',
            'source_id': 'oh_ir1_gw',
            'bucket_start': bucket_start,
            'gap': True,
        },
    ]

    jobs = pipeline_v2.build_nfcapd_logical_bucket_jobs(
        conn,
        [
            pipeline_v2.SourceDefinition(
                source_id='uoregon_all',
                members=('cc_ir1_gw', 'oh_ir1_gw'),
            )
        ],
        member_specs,
    )

    assert jobs == [
        {
            'source_id': 'uoregon_all',
            'bucket_start': bucket_start,
            'bucket_end': bucket_start + 300,
            'member_specs': [],
            'missing_members': ['cc_ir1_gw', 'oh_ir1_gw'],
        }
    ]


def test_process_nfcapd_logical_bucket_jobs_rewrites_empty_union_gap() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    bucket_start = 1738396800

    pipeline_v2.process_nfcapd_logical_bucket_jobs(
        conn,
        [
            {
                'source_id': 'uoregon_all',
                'bucket_start': bucket_start,
                'bucket_end': bucket_start + 300,
                'member_specs': [],
                'missing_members': ['cc_ir1_gw', 'oh_ir1_gw'],
            }
        ],
        max_workers=1,
        run_maad=False,
    )

    assert conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows FROM netflow_stats_v2 ORDER BY ip_version'
    ).fetchall() == [
        ('uoregon_all', bucket_start, 4, 0),
        ('uoregon_all', bucket_start, 6, 0),
    ]
    assert conn.execute(
        'SELECT input_locator, source_id, status FROM processed_inputs_v2'
    ).fetchall() == [
        ('gap://nfcapd/uoregon_all/202502010000', 'uoregon_all', 'processed')
    ]


def test_process_nfcapd_logical_bucket_jobs_merges_member_payloads(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    bucket_start = 1738396800
    bucket_end = bucket_start + 300

    def raw_bucket(path: str, member_id: str, src_ip: str, dst_ip: str, flows: int) -> dict:
        netflow_row = pipeline_v2.new_netflow_bucket_from_values(
            source_id=member_id,
            bucket_start=bucket_start,
            bucket_end=bucket_end,
            ip_version=4,
        )
        netflow_row['flows'] = flows
        netflow_row['flows_tcp'] = flows
        netflow_row['packets'] = flows * 10
        netflow_row['packets_tcp'] = flows * 10
        netflow_row['bytes'] = flows * 100
        netflow_row['bytes_tcp'] = flows * 100
        return {
            'processed_buckets': [
                {
                    'input_kind': 'nfcapd',
                    'input_locator': path,
                    'source_id': member_id,
                    'bucket_start': bucket_start,
                    'bucket_end': bucket_end,
                }
            ],
            'raw_buckets': [
                {
                    'source_id': member_id,
                    'bucket_start': bucket_start,
                    'source_ipv4': [src_ip],
                    'destination_ipv4': [dst_ip],
                    'source_ipv6': [],
                    'destination_ipv6': [],
                    'protocols_ipv4': ['6'],
                    'protocols_ipv6': [],
                    'maad_source_ipv4': [src_ip],
                    'maad_destination_ipv4': [dst_ip],
                    'netflow_rows': [netflow_row],
                }
            ],
        }

    payloads = {
        '/captures/cc/nfcapd.202502010000': raw_bucket(
            '/captures/cc/nfcapd.202502010000',
            'cc_ir1_gw',
            '192.0.2.1',
            '198.51.100.1',
            2,
        ),
        '/captures/oh/nfcapd.202502010000': raw_bucket(
            '/captures/oh/nfcapd.202502010000',
            'oh_ir1_gw',
            '192.0.2.2',
            '198.51.100.2',
            3,
        ),
    }

    def fake_build_input_payload(spec, maad_bin, maad_backend='subprocess', maad_workers=1, run_maad=True):
        return payloads[str(spec['path'])]

    monkeypatch.setattr(pipeline_v2, 'build_input_payload', fake_build_input_payload)

    pipeline_v2.process_nfcapd_logical_bucket_jobs(
        conn,
        [
            {
                'source_id': 'uoregon_all',
                'bucket_start': bucket_start,
                'bucket_end': bucket_end,
                'member_specs': [
                    {'input_kind': 'nfcapd', 'path': '/captures/cc/nfcapd.202502010000', 'source_id': 'cc_ir1_gw'},
                    {'input_kind': 'nfcapd', 'path': '/captures/oh/nfcapd.202502010000', 'source_id': 'oh_ir1_gw'},
                ],
                'missing_members': [],
            }
        ],
        max_workers=1,
        run_maad=False,
    )

    assert conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, packets, bytes FROM netflow_stats_v2'
    ).fetchall() == [('uoregon_all', bucket_start, 4, 5, 50, 500)]
    assert conn.execute(
        "SELECT source_id, granularity, sa_ipv4_count, da_ipv4_count FROM ip_stats_v2 WHERE granularity = '5m'"
    ).fetchall() == [('uoregon_all', '5m', 2, 2)]
    assert conn.execute(
        'SELECT input_locator, source_id, status FROM processed_inputs_v2 ORDER BY input_locator'
    ).fetchall() == [
        ('/captures/cc/nfcapd.202502010000', 'uoregon_all', 'processed'),
        ('/captures/oh/nfcapd.202502010000', 'uoregon_all', 'processed'),
    ]


def test_logical_5m_maad_uses_maad_workers(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    calls = []

    def fake_process_maad_tasks(tasks, maad_workers):
        calls.append((tasks, maad_workers))
        return []

    monkeypatch.setattr(pipeline_v2, 'process_maad_tasks', fake_process_maad_tasks)

    pipeline_v2.build_5m_maad_rows_from_raw_buckets(
        [
            {
                'source_id': 'uoregon_all',
                'bucket_start': 1738396800,
                'maad_source_ipv4': ['192.0.2.1'],
                'maad_destination_ipv4': ['198.51.100.1'],
            }
        ],
        maad_bin='/tmp/maad',
        maad_backend='subprocess',
        maad_workers=1,
    )

    assert calls[0][1] == 1


def test_process_input_specs_writes_zero_rows_for_nfcapd_gap() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    bucket_start = 1744733100

    pipeline_v2.process_input_specs(
        conn,
        [
            {
                'input_kind': 'nfcapd',
                'path': 'gap://nfcapd/oh_ir1_gw/202504150005',
                'source_id': 'oh_ir1_gw',
                'bucket_start': bucket_start,
                'gap': True,
            }
        ],
        max_workers=1,
        run_maad=False,
    )

    processed_inputs = conn.execute(
        'SELECT input_kind, input_locator, source_id, bucket_start, status FROM processed_inputs_v2'
    ).fetchall()
    netflow = conn.execute(
        'SELECT source_id, bucket_start, ip_version, flows, packets, bytes FROM netflow_stats_v2 ORDER BY ip_version'
    ).fetchall()
    aggregate_netflow = conn.execute(
        """
        SELECT granularity, ip_version, flows, packets, bytes
        FROM netflow_stats_aggregate_v2
        ORDER BY granularity, ip_version
        """
    ).fetchall()
    ip_stats = conn.execute(
        "SELECT granularity, sa_ipv4_count, da_ipv4_count, sa_ipv6_count, da_ipv6_count FROM ip_stats_v2 ORDER BY granularity"
    ).fetchall()
    protocol_stats = conn.execute(
        "SELECT granularity, protocols_list_ipv4, protocols_list_ipv6 FROM protocol_stats_v2 ORDER BY granularity"
    ).fetchall()

    assert processed_inputs == [
        ('nfcapd', 'gap://nfcapd/oh_ir1_gw/202504150005', 'oh_ir1_gw', bucket_start, 'processed')
    ]
    assert netflow == [
        ('oh_ir1_gw', bucket_start, 4, 0, 0, 0),
        ('oh_ir1_gw', bucket_start, 6, 0, 0, 0),
    ]
    assert aggregate_netflow == [
        ('1d', 4, 0, 0, 0),
        ('1d', 6, 0, 0, 0),
        ('1h', 4, 0, 0, 0),
        ('1h', 6, 0, 0, 0),
        ('30m', 4, 0, 0, 0),
        ('30m', 6, 0, 0, 0),
    ]
    assert ip_stats == [
        ('1d', 0, 0, 0, 0),
        ('1h', 0, 0, 0, 0),
        ('30m', 0, 0, 0, 0),
        ('5m', 0, 0, 0, 0),
    ]
    assert protocol_stats == [
        ('1d', '', ''),
        ('1h', '', ''),
        ('30m', '', ''),
        ('5m', '', ''),
    ]


def test_process_input_specs_always_runs_maad(monkeypatch) -> None:
    pipeline_v2, normalized_rows_v2 = load_modules()
    conn = sqlite3.connect(':memory:')

    def fake_iter_csv_rows(path: str, mapping_path: str):
        assert path == '/captures/flows.csv'
        assert mapping_path == '/captures/mapping.json'
        return iter(
            [
                normalized_rows_v2.NormalizedRow(
                    source_id='oh_ir1_gw',
                    bucket_start=1744733100,
                    bucket_end=1744733400,
                    time_received=1744733279,
                    time_end=1744733000,
                    time_start=1744732700,
                    src_ip='192.0.2.1',
                    dst_ip='198.51.100.9',
                    ip_version=4,
                    src_port=443,
                    dst_port=55000,
                    protocol=6,
                    packets=10,
                    bytes=1000,
                    src_tos=2,
                    dst_tos=0,
                ),
                normalized_rows_v2.NormalizedRow(
                    source_id='oh_ir1_gw',
                    bucket_start=1744733100,
                    bucket_end=1744733400,
                    time_received=1744733279,
                    time_end=1744733000,
                    time_start=1744732700,
                    src_ip='192.0.2.2',
                    dst_ip='198.51.100.10',
                    ip_version=4,
                    src_port=443,
                    dst_port=55001,
                    protocol=6,
                    packets=11,
                    bytes=1100,
                    src_tos=2,
                    dst_tos=0,
                ),
            ]
        )

    maad_calls = []

    def fake_run_maad_json(maad_bin, addresses, *, timeout_seconds):
        maad_calls.append((str(maad_bin), sorted(addresses)))
        return pipeline_v2.MaadJsonResult(
            schema_version=1,
            metadata={
                'input': '-',
                'minPrefixLength': 7,
                'maxPrefixLength': 23,
                'totalAddrs': len(addresses),
            },
            structure=[{'q': -0.5, 'tauTilde': -0.98, 'sd': 0.01}],
            spectrum=[{'alpha': 0.75, 'f': 0.60}],
            dimensions=[{'q': 1, 'dim': 0.48}],
        )

    monkeypatch.setattr(pipeline_v2, 'iter_csv_rows', fake_iter_csv_rows)
    monkeypatch.setattr(pipeline_v2, 'run_maad_json', fake_run_maad_json)
    monkeypatch.setattr(pipeline_v2, 'should_stream_csv_input_specs', lambda specs: True)
    monkeypatch.setattr(
        pipeline_v2,
        'load_csv_source_config',
        lambda path: type('Config', (), {'out_of_order_lag_buckets': 12})(),
    )

    pipeline_v2.process_input_specs(
        conn,
        [
                {
                    'input_kind': 'csv',
                    'path': '/captures/flows.csv',
                    'mapping_path': '/captures/mapping.json',
                }
        ],
        maad_bin='/tmp/MAAD',
        max_workers=1,
    )

    structure = conn.execute(
        "SELECT source_id, bucket_start, ip_version, structure_json_sa, structure_json_da FROM structure_stats_v2 WHERE granularity = '5m'"
    ).fetchone()
    processed_inputs = conn.execute(
        'SELECT status FROM processed_inputs_v2'
    ).fetchone()

    assert maad_calls[:2] == [
        ('/tmp/MAAD', ['192.0.2.1', '192.0.2.2']),
        ('/tmp/MAAD', ['198.51.100.10', '198.51.100.9']),
    ]
    assert structure[:3] == ('oh_ir1_gw', 1744733100, 4)
    assert processed_inputs == ('processed',)


def test_fractional_nfdump_timestamp_rows_are_not_headers() -> None:
    pipeline_v2, _ = load_modules()

    assert not pipeline_v2.looks_like_nfdump_header(['1741075200.000'])
    assert pipeline_v2.looks_like_nfdump_header(['trr', 'ter', 'tsr'])


def test_unexpected_nfdump_text_rows_are_not_treated_as_headers(caplog) -> None:
    pipeline_v2, _ = load_modules()

    assert not pipeline_v2.looks_like_nfdump_header(['not-a-timestamp'])
    assert 'Malformed nfdump CSV row' in caplog.text


def test_write_input_payload_rolls_back_stats_on_failure(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    pipeline_v2.init_processed_inputs_v2_table(conn)
    pipeline_v2.init_netflow_stats_v2_table(conn)
    pipeline_v2.init_ip_stats_v2_table(conn)
    pipeline_v2.init_protocol_stats_v2_table(conn)

    payload = {
        'processed_buckets': [
            {
                'input_kind': 'csv',
                'input_locator': '/tmp/flows.csv',
                'source_id': 'feed-a',
                'bucket_start': 1744732800,
                'bucket_end': 1744733100,
            }
        ],
        'netflow_rows': [
            {
                'source_id': 'feed-a',
                'bucket_start': 1744732800,
                'bucket_end': 1744733100,
                'ip_version': 4,
                'flows': 1,
                'flows_tcp': 1,
                'flows_udp': 0,
                'flows_icmp': 0,
                'flows_other': 0,
                'packets': 1,
                'packets_tcp': 1,
                'packets_udp': 0,
                'packets_icmp': 0,
                'packets_other': 0,
                'bytes': 1,
                'bytes_tcp': 1,
                'bytes_udp': 0,
                'bytes_icmp': 0,
                'bytes_other': 0,
            }
        ],
        'ip_rows': [],
        'protocol_rows': [],
        'maad_rows': [],
    }

    def fail_ip_insert(conn, rows):
        raise RuntimeError('ip insert failed')

    monkeypatch.setattr(pipeline_v2, 'insert_ip_stats_v2_rows', fail_ip_insert)

    with pytest.raises(RuntimeError, match='ip insert failed'):
        pipeline_v2.write_input_payload(conn, payload)

    assert conn.execute('SELECT COUNT(*) FROM processed_inputs_v2').fetchone()[0] == 0
    assert conn.execute('SELECT COUNT(*) FROM netflow_stats_v2').fetchone()[0] == 0


def test_csv_input_fully_processed_requires_only_processed_rows() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    pipeline_v2.init_processed_inputs_v2_table(conn)

    assert not pipeline_v2.csv_input_fully_processed(conn, '/tmp/flows.csv')

    pipeline_v2.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator='/tmp/flows.csv',
        source_id='feed-a',
        bucket_start=1744732800,
        bucket_end=1744733100,
    )
    assert not pipeline_v2.csv_input_fully_processed(conn, '/tmp/flows.csv')

    pipeline_v2.mark_input_bucket_status(
        conn,
        input_kind='csv',
        input_locator='/tmp/flows.csv',
        source_id='feed-a',
        bucket_start=1744732800,
        status='processed',
    )
    assert pipeline_v2.csv_input_fully_processed(conn, '/tmp/flows.csv')

    pipeline_v2.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator='/tmp/flows.csv',
        source_id='feed-a',
        bucket_start=1744733100,
        bucket_end=1744733400,
    )
    assert not pipeline_v2.csv_input_fully_processed(conn, '/tmp/flows.csv')


def test_mark_csv_buckets_waits_until_daily_aggregate_is_flushed() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    pipeline_v2.init_processed_inputs_v2_table(conn)
    day_start = int(datetime(2025, 4, 15, tzinfo=pipeline_v2.PIPELINE_TIMEZONE).timestamp())
    bucket = {
        'input_kind': 'csv',
        'input_locator': '/tmp/flows.csv',
        'source_id': 'feed-a',
        'bucket_start': day_start,
        'bucket_end': day_start + 300,
    }
    pipeline_v2.upsert_input_bucket(conn, **bucket)
    pending = [bucket]

    pipeline_v2.mark_csv_buckets_with_flushed_aggregates(conn, pending, day_start + 3600)

    assert conn.execute('SELECT status FROM processed_inputs_v2').fetchone() == ('pending',)
    assert pending == [bucket]

    pipeline_v2.mark_csv_buckets_with_flushed_aggregates(conn, pending, day_start + 86400)

    assert conn.execute('SELECT status FROM processed_inputs_v2').fetchone() == ('processed',)
    assert pending == []

    db_only_bucket = {
        'input_kind': 'csv',
        'input_locator': '/tmp/flows.csv',
        'source_id': 'feed-a',
        'bucket_start': day_start + 300,
        'bucket_end': day_start + 600,
    }
    pipeline_v2.upsert_input_bucket(conn, **db_only_bucket)

    pipeline_v2.mark_csv_buckets_with_flushed_aggregates(conn, [], day_start + 86400)

    assert conn.execute(
        'SELECT status FROM processed_inputs_v2 WHERE bucket_start = ?',
        (day_start + 300,),
    ).fetchone() == ('processed',)


def test_skip_processed_csv_bucket_values_filters_retry_buckets() -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    pipeline_v2.init_processed_inputs_v2_table(conn)
    processed_bucket = pipeline_v2.BucketAccumulator(
        source_id='feed-a',
        bucket_start=1744732800,
        bucket_end=1744733100,
    )
    pending_bucket = pipeline_v2.BucketAccumulator(
        source_id='feed-a',
        bucket_start=1744733100,
        bucket_end=1744733400,
    )
    pipeline_v2.upsert_input_bucket(
        conn,
        input_kind='csv',
        input_locator='/tmp/flows.csv',
        source_id='feed-a',
        bucket_start=1744732800,
        bucket_end=1744733100,
    )
    pipeline_v2.mark_input_bucket_status(
        conn,
        input_kind='csv',
        input_locator='/tmp/flows.csv',
        source_id='feed-a',
        bucket_start=1744732800,
        status='processed',
    )

    remaining = pipeline_v2.skip_processed_csv_bucket_values(
        conn,
        '/tmp/flows.csv',
        [processed_bucket, pending_bucket],
    )

    assert remaining == [pending_bucket]


def test_process_input_specs_can_write_parallel_worker_payloads(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    conn = sqlite3.connect(':memory:')
    captured_processes = []

    payloads = [
        {
            'processed_buckets': [
                {
                    'input_kind': 'csv',
                    'input_locator': '/tmp/a.csv',
                    'source_id': 'feed-a',
                    'bucket_start': 1744732800,
                    'bucket_end': 1744733100,
                }
            ],
            'netflow_rows': [],
            'ip_rows': [
                    {
                        'source_id': 'feed-a',
                        'granularity': '5m',
                        'bucket_start': 1744732800,
                    'bucket_end': 1744733100,
                    'sa_ipv4_count': 1,
                    'da_ipv4_count': 1,
                    'sa_ipv6_count': 0,
                    'da_ipv6_count': 0,
                }
            ],
            'protocol_rows': [],
            'maad_rows': [],
        },
        {
            'processed_buckets': [
                {
                    'input_kind': 'csv',
                    'input_locator': '/tmp/b.csv',
                    'source_id': 'feed-b',
                    'bucket_start': 1744733100,
                    'bucket_end': 1744733400,
                }
            ],
            'netflow_rows': [],
            'ip_rows': [
                    {
                        'source_id': 'feed-b',
                        'granularity': '5m',
                        'bucket_start': 1744733100,
                    'bucket_end': 1744733400,
                    'sa_ipv4_count': 2,
                    'da_ipv4_count': 2,
                    'sa_ipv6_count': 0,
                    'da_ipv6_count': 0,
                }
            ],
            'protocol_rows': [],
            'maad_rows': [],
        },
    ]

    class FakePool:
        def __init__(self, processes):
            captured_processes.append(processes)

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def imap_unordered(self, worker, tasks, chunksize=1):
            assert chunksize == 1
            return iter(payloads)

    monkeypatch.setattr(pipeline_v2, 'Pool', FakePool)
    monkeypatch.setattr(
        pipeline_v2,
        'process_input_spec_worker',
        lambda task: (_ for _ in ()).throw(AssertionError('fake pool should supply payloads')),
        raising=False,
    )

    pipeline_v2.process_input_specs(
        conn,
        [
            {'input_kind': 'csv', 'path': '/tmp/a.csv', 'mapping_path': '/tmp/a.json'},
            {'input_kind': 'csv', 'path': '/tmp/b.csv', 'mapping_path': '/tmp/b.json'},
        ],
        max_workers=2,
    )

    assert captured_processes == [2]
    assert conn.execute(
        'SELECT source_id, bucket_start, sa_ipv4_count FROM ip_stats_v2 ORDER BY source_id'
    ).fetchall() == [
        ('feed-a', 1744732800, 1),
        ('feed-b', 1744733100, 2),
    ]


def test_process_input_specs_writes_v1_granularity_aggregates_for_csv(monkeypatch, tmp_path: Path) -> None:
    pipeline_v2, _ = load_modules()
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
                'source_id': {'value': 'uo-feed'},
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
    conn = sqlite3.connect(':memory:')

    def fake_run_maad_json(maad_bin, addresses, *, timeout_seconds):
        return pipeline_v2.MaadJsonResult(
            schema_version=1,
            metadata={
                'input': '-',
                'minPrefixLength': 7,
                'maxPrefixLength': 23,
                'totalAddrs': len(addresses),
            },
            structure=[],
            spectrum=[],
            dimensions=[],
        )

    monkeypatch.setattr(pipeline_v2, 'run_maad_json', fake_run_maad_json)

    pipeline_v2.process_input_specs(
        conn,
        [
            {
                'input_kind': 'csv',
                'path': str(csv_path),
                'mapping_path': str(mapping_path),
            }
        ],
        maad_bin='/tmp/MAAD',
        max_workers=1,
    )

    ip_stats = conn.execute(
        """
        SELECT granularity, bucket_start, sa_ipv4_count, da_ipv4_count
        FROM ip_stats_v2
        ORDER BY granularity, bucket_start
        """
    ).fetchall()
    protocol_stats = conn.execute(
        """
        SELECT granularity, bucket_start, unique_protocols_count_ipv4, protocols_list_ipv4
        FROM protocol_stats_v2
        ORDER BY granularity, bucket_start
        """
    ).fetchall()
    netflow_aggregate_stats = conn.execute(
        """
        SELECT granularity, bucket_start, ip_version, flows, flows_tcp, flows_udp, packets, bytes
        FROM netflow_stats_aggregate_v2
        ORDER BY granularity, bucket_start, ip_version
        """
    ).fetchall()
    maad_30m = conn.execute(
        """
        SELECT json_extract(metadata_json_sa, '$.totalAddrs'),
               json_extract(metadata_json_da, '$.totalAddrs')
        FROM structure_stats_v2
        WHERE granularity = '30m'
        """
    ).fetchone()

    assert ip_stats == [
        ('1d', 1744700400, 2, 2),
        ('1h', 1744732800, 2, 2),
        ('30m', 1744732800, 2, 2),
        ('5m', 1744732800, 1, 1),
        ('5m', 1744733100, 1, 1),
    ]
    assert protocol_stats == [
        ('1d', 1744700400, 2, '17,6'),
        ('1h', 1744732800, 2, '17,6'),
        ('30m', 1744732800, 2, '17,6'),
        ('5m', 1744732800, 1, '6'),
        ('5m', 1744733100, 1, '17'),
    ]
    assert netflow_aggregate_stats == [
        ('1d', 1744700400, 4, 2, 1, 1, 30, 3000),
        ('1h', 1744732800, 4, 2, 1, 1, 30, 3000),
        ('30m', 1744732800, 4, 2, 1, 1, 30, 3000),
    ]
    assert maad_30m == (2, 2)


def test_process_maad_row_task_uses_timeout_by_granularity(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    timeouts = []

    def fake_run_maad_json(maad_bin, addresses, *, timeout_seconds):
        timeouts.append(timeout_seconds)
        return pipeline_v2.MaadJsonResult(
            schema_version=1,
            metadata={
                'input': '-',
                'minPrefixLength': 7,
                'maxPrefixLength': 23,
                'totalAddrs': len(addresses),
            },
            structure=[],
            spectrum=[],
            dimensions=[],
        )

    monkeypatch.setattr(pipeline_v2, 'run_maad_json', fake_run_maad_json)

    rows = pipeline_v2.process_maad_row_task(
        {
            'maad_bin': '/tmp/MAAD',
            'source_id': 'oh_ir1_gw',
            'granularity': '1d',
            'bucket_start': 1749625200,
            'bucket_end': 1749711600,
            'source_addresses': ['192.0.2.1', '192.0.2.2'],
            'destination_addresses': ['198.51.100.1', '198.51.100.2', '198.51.100.3'],
            'log_progress': False,
        }
    )

    assert timeouts == [1800, 1800]
    assert json.loads(rows['structure']['metadata_json_sa'])['totalAddrs'] == 2
    assert json.loads(rows['structure']['metadata_json_da'])['totalAddrs'] == 3


def test_build_aggregate_maad_rows_caps_pool_size(monkeypatch) -> None:
    pipeline_v2, _ = load_modules()
    captured_processes = []

    class FakePool:
        def __init__(self, processes):
            captured_processes.append(processes)

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

        def imap_unordered(self, worker, tasks, chunksize=1):
            return iter(worker(task) for task in tasks)

    monkeypatch.setattr(pipeline_v2, 'Pool', FakePool)
    monkeypatch.setattr(
        pipeline_v2,
        'process_maad_row_task',
        lambda task: {'structure': {'source_id': task['source_id']}},
    )

    rows = pipeline_v2.build_aggregate_maad_rows(
        [
            {
                'source_id': 'feed-a',
                'bucket_start': 1744732800,
                'maad_source_ipv4': ['192.0.2.1'],
                'maad_destination_ipv4': ['198.51.100.1'],
            },
            {
                'source_id': 'feed-a',
                'bucket_start': 1744733100,
                'maad_source_ipv4': ['192.0.2.2'],
                'maad_destination_ipv4': ['198.51.100.2'],
            },
        ],
        '/tmp/MAAD',
        'subprocess',
        maad_workers=16,
        max_workers=16,
    )

    assert captured_processes == [4]
    assert len(rows) == 3


def test_aggregate_day_bucket_end_uses_local_midnight_across_dst(monkeypatch) -> None:
    monkeypatch.setenv('NETFLOW_TIMEZONE', 'America/Los_Angeles')
    pipeline_v2, _ = load_modules()
    timezone = ZoneInfo('America/Los_Angeles')
    spring_start = int(datetime(2025, 3, 9, 1, 0, tzinfo=timezone).timestamp())
    spring_day_start = int(datetime(2025, 3, 9, 0, 0, tzinfo=timezone).timestamp())
    spring_next_day = int(datetime(2025, 3, 10, 0, 0, tzinfo=timezone).timestamp())
    fall_start = int(datetime(2025, 11, 2, 0, 30, tzinfo=timezone).timestamp())
    fall_day_start = int(datetime(2025, 11, 2, 0, 0, tzinfo=timezone).timestamp())
    fall_next_day = int(datetime(2025, 11, 3, 0, 0, tzinfo=timezone).timestamp())

    rows = pipeline_v2.build_aggregate_ip_rows(
        [
            {
                'source_id': 'feed-a',
                'bucket_start': spring_start,
                'source_ipv4': ['192.0.2.1'],
                'destination_ipv4': ['198.51.100.1'],
                'source_ipv6': [],
                'destination_ipv6': [],
            },
            {
                'source_id': 'feed-a',
                'bucket_start': fall_start,
                'source_ipv4': ['192.0.2.2'],
                'destination_ipv4': ['198.51.100.2'],
                'source_ipv6': [],
                'destination_ipv6': [],
            },
        ]
    )
    day_bounds = {
        row['bucket_start']: row['bucket_end'] for row in rows if row['granularity'] == '1d'
    }

    assert day_bounds == {
        spring_day_start: spring_next_day,
        fall_day_start: fall_next_day,
    }
