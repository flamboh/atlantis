import importlib
from pathlib import Path

import pytest


def load_modules():
    csv_ingest = importlib.import_module('csv_ingest')
    normalized_rows = importlib.import_module('normalized_rows')
    return importlib.reload(csv_ingest), importlib.reload(normalized_rows)


def test_normalize_csv_row_uses_time_received_and_infers_ipv4(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_received": "received_at",
            "time_end": "ended_at",
            "time_start": "started_at",
            "src_ip": "src",
            "dst_ip": "dst",
            "src_port": "sp",
            "dst_port": "dp",
            "protocol": "pr",
            "packets": "pkt",
            "bytes": "byt",
            "src_tos": "stos",
            "dst_tos": "dtos"
          },
          "source_id": { "value": "uo-feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    row = normalized_rows.normalize_csv_row(
        {
            'received_at': '1744733279',
            'ended_at': '1744733000',
            'started_at': '1744732700',
            'src': '192.0.2.1',
            'dst': '198.51.100.9',
            'sp': '443',
            'dp': '55000',
            'pr': '6',
            'pkt': '10',
            'byt': '2048',
            'stos': '2',
            'dtos': '0',
        },
        config,
    )

    assert row.source_id == 'uo-feed'
    assert row.bucket_start == 1744733100
    assert row.bucket_end == 1744733400
    assert row.ip_version == 4
    assert row.src_port == 443
    assert row.dst_port == 55000
    assert row.protocol == 6
    assert row.packets == 10
    assert row.bytes == 2048
    assert row.src_tos == 2
    assert row.dst_tos == 0


def test_normalize_csv_row_infers_ipv6_and_defaults_optional_fields(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_end": "ended_at",
            "src_ip": "src",
            "dst_ip": "dst"
          },
          "source_id": { "column": "source_name" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    row = normalized_rows.normalize_csv_row(
        {
            'ended_at': '1744733000',
            'src': '2001:db8::1',
            'dst': '2001:db8::2',
            'source_name': 'oh_ir1_gw',
        },
        config,
    )

    assert row.source_id == 'oh_ir1_gw'
    assert row.ip_version == 6
    assert row.bucket_start == 1744732800
    assert row.src_port == 0
    assert row.dst_port == 0
    assert row.protocol == 0
    assert row.packets == 0
    assert row.bytes == 0
    assert row.src_tos == 0
    assert row.dst_tos == 0


def test_normalize_csv_row_wraps_invalid_ip_as_config_error(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_received": "received_at",
            "src_ip": "src",
            "dst_ip": "dst"
          },
          "source_id": { "value": "uo-feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    with pytest.raises(csv_ingest.CsvSourceConfigError, match='Invalid IP address'):
        normalized_rows.normalize_csv_row(
            {
                'received_at': '1744733279',
                'src': 'not-an-ip',
                'dst': '198.51.100.9',
            },
            config,
        )

    with pytest.raises(csv_ingest.CsvSourceConfigError, match='Invalid IP address'):
        normalized_rows.normalize_csv_row(
            {
                'received_at': '1744733279',
                'src': '999.999.999.999',
                'dst': '198.51.100.9',
            },
            config,
        )


def test_normalize_csv_row_rejects_whitespace_required_values(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_received": "received_at",
            "src_ip": "src",
            "dst_ip": "dst"
          },
          "source_id": { "value": "uo-feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    with pytest.raises(csv_ingest.CsvSourceConfigError, match='src'):
        normalized_rows.normalize_csv_row(
            {
                'received_at': '1744733279',
                'src': '   ',
                'dst': '198.51.100.9',
            },
            config,
        )


def test_build_nfdump_csv_command_uses_time_received_and_family_filter() -> None:
    _, normalized_rows = load_modules()

    command = normalized_rows.build_nfdump_csv_command(
        '/captures/r1/2025/04/15/nfcapd.202504150000',
        ip_version=6,
    )

    assert command[:4] == ['nfdump', '-r', '/captures/r1/2025/04/15/nfcapd.202504150000', '-q']
    assert command[4:6] == ['-o', 'csv:%trr,%ter,%tsr,%sa,%da,%sp,%dp,%pr,%pkt,%byt,%stos,%dtos']
    assert command[-2:] == ['ipv6', '-6']


def test_normalize_nfdump_csv_values_maps_expected_column_order() -> None:
    _, normalized_rows = load_modules()

    row = normalized_rows.normalize_nfdump_csv_values(
        [
            '1744733279.999',
            '1744733000.001',
            '1744732700.500',
            '192.0.2.1',
            '198.51.100.9',
            '443',
            '55000',
            '6',
            '10',
            '2048',
            '2',
            '0',
        ],
        source_id='oh_ir1_gw',
    )

    assert row.source_id == 'oh_ir1_gw'
    assert row.bucket_start == 1744733100
    assert row.time_received == 1744733279
    assert row.time_end == 1744733000
    assert row.time_start == 1744732700
    assert row.ip_version == 4


def test_normalize_nfdump_csv_values_zeroes_decimal_pseudo_ports() -> None:
    _, normalized_rows = load_modules()

    row = normalized_rows.normalize_nfdump_csv_values(
        [
            '1744733279.000',
            '1744733000.000',
            '1744732700.000',
            '192.0.2.1',
            '198.51.100.9',
            '0',
            '3.1',
            '1',
            '10',
            '2048',
            '2',
            '0',
        ],
        source_id='oh_ir1_gw',
    )

    assert row.dst_port == 0


def test_normalize_csv_row_accepts_protocol_names(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "datetime",
          "timestamp_timezone": "Europe/Madrid",
          "columns": {
            "time_end": "te",
            "src_ip": "src",
            "dst_ip": "dst",
            "src_port": "sp",
            "dst_port": "dp",
            "protocol": "pr",
            "packets": "pkt",
            "bytes": "byt"
          },
          "protocol_map": { "UDP": 17 },
          "source_id": { "value": "ugr16" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    row = normalized_rows.normalize_csv_row(
        {
            'te': '2016-07-27 13:43:30',
            'src': '42.219.154.107',
            'dst': '143.72.8.137',
            'sp': '59212',
            'dp': '53',
            'pr': '  uDp  ',
            'pkt': '1',
            'byt': '72',
        },
        config,
    )

    assert row.bucket_start == 1469619600
    assert row.protocol == 17
    assert row.packets == 1
    assert row.bytes == 72


@pytest.mark.parametrize(
    ('column', 'value'),
    [
        ('pr', '-1'),
        ('pr', '256'),
        ('pkt', '-1'),
        ('pkt', str(1 << 63)),
        ('byt', '-1'),
        ('byt', str(1 << 63)),
        ('stos', '-1'),
        ('stos', '256'),
        ('dtos', '-1'),
        ('dtos', '256'),
    ],
)
def test_normalize_csv_row_rejects_out_of_range_flow_values(
    tmp_path: Path,
    column: str,
    value: str,
) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_end": "te",
            "src_ip": "src",
            "dst_ip": "dst",
            "protocol": "pr",
            "packets": "pkt",
            "bytes": "byt",
            "src_tos": "stos",
            "dst_tos": "dtos"
          },
          "source_id": { "value": "feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)
    row = {
        'te': '1744733279',
        'src': '192.0.2.1',
        'dst': '198.51.100.1',
        'pr': '6',
        'pkt': '1',
        'byt': '1',
        'stos': '0',
        'dtos': '0',
    }
    row[column] = value

    with pytest.raises(csv_ingest.CsvSourceConfigError, match='must be'):
        normalized_rows.normalize_csv_row(row, config)


def test_normalize_csv_row_strips_optional_numeric_whitespace(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "timestamp_format": "unix",
          "columns": {
            "time_end": "te",
            "src_ip": "src",
            "dst_ip": "dst",
            "protocol": "pr",
            "packets": "pkt",
            "bytes": "byt",
            "src_tos": "stos"
          },
          "source_id": { "value": "feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)

    row = normalized_rows.normalize_csv_row(
        {
            'te': ' 1744733279 ',
            'src': ' 192.0.2.1 ',
            'dst': ' 198.51.100.1 ',
            'pr': ' 6 ',
            'pkt': ' 1 ',
            'byt': ' 2 ',
            'stos': ' 3 ',
        },
        config,
    )

    assert (row.protocol, row.packets, row.bytes, row.src_tos) == (6, 1, 2, 3)
