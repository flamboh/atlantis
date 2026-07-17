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
    assert row.observation.ip_version == 4
    assert row.observation.src_port == 443
    assert row.observation.dst_port == 55000
    assert row.observation.protocol == 6
    assert row.observation.packets == 10
    assert row.observation.bytes_count == 2048
    assert row.observation.src_tos == 2
    assert row.observation.dst_tos == 0


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
    assert row.observation.ip_version == 6
    assert row.bucket_start == 1744732800
    assert row.observation.src_port is None
    assert row.observation.dst_port is None
    assert row.observation.protocol == 0
    assert row.observation.packets == 0
    assert row.observation.bytes_count == 0
    assert row.observation.src_tos == 0
    assert row.observation.dst_tos == 0


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
    assert row.observation.time_received_ms == 1744733279999
    assert row.observation.time_end_ms == 1744733000001
    assert row.observation.time_start_ms == 1744732700500
    assert row.observation.ip_version == 4


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

    assert row.observation.dst_port == 0


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
    assert row.observation.protocol == 17
    assert row.observation.packets == 1
    assert row.observation.bytes_count == 72


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

    assert (
        row.observation.protocol,
        row.observation.packets,
        row.observation.bytes_count,
        row.observation.src_tos,
    ) == (6, 1, 2, 3)


def test_optional_observation_values_distinguish_missing_from_zero(tmp_path: Path) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    config_path.write_text(
        """
        {
          "has_header": false,
          "fieldnames": ["te", "src", "dst", "sp", "dp", "duration", "min_ttl", "max_ttl"],
          "columns": {
            "time_end": "te",
            "src_ip": "src",
            "dst_ip": "dst",
            "src_port": "sp",
            "dst_port": "dp",
            "duration": "duration",
            "min_ttl": "min_ttl",
            "max_ttl": "max_ttl"
          },
          "source_id": { "value": "feed" }
        }
        """,
        encoding='utf-8',
    )
    config = csv_ingest.load_csv_source_config(config_path)
    indexes = {name: index for index, name in enumerate(config.fieldnames or [])}

    missing = normalized_rows.normalize_csv_values(
        ['1744733279', '192.0.2.1', '198.51.100.1', '', '', '', '', ''],
        config,
        indexes,
    ).observation
    zero = normalized_rows.normalize_csv_values(
        ['1744733279', '192.0.2.1', '198.51.100.1', '0', '0', '0', '0', '0'],
        config,
        indexes,
    ).observation

    assert (missing.src_port, missing.dst_port, missing.duration_ms) == (None, None, None)
    assert (missing.min_ttl, missing.max_ttl) == (None, None)
    assert (zero.src_port, zero.dst_port, zero.duration_ms) == (0, 0, 0)
    assert (zero.min_ttl, zero.max_ttl) == (0, 0)


def test_mapped_duration_seconds_are_authoritative_over_endpoints() -> None:
    _, normalized_rows = load_modules()

    duration_ms = normalized_rows.resolve_duration_ms(
        '1.234',
        'duration',
        {'time_start': 2000, 'time_end': 1000},
    )

    assert duration_ms == 1234


def test_derived_duration_uses_signed_64_bit_bound() -> None:
    csv_ingest, normalized_rows = load_modules()
    maximum = normalized_rows.MAX_SQLITE_INTEGER

    assert (
        normalized_rows.resolve_duration_ms(
            None,
            None,
            {'time_start': 0, 'time_end': maximum},
        )
        == maximum
    )
    with pytest.raises(csv_ingest.CsvSourceConfigError, match=f'0..{maximum}'):
        normalized_rows.resolve_duration_ms(
            None,
            None,
            {'time_start': -1, 'time_end': maximum},
        )


@pytest.mark.parametrize('adapter', ['mapping', 'indexed'])
@pytest.mark.parametrize(
    ('column', 'value', 'message'),
    [
        ('sp', '65536', 'must be'),
        ('dp', '-1', 'must be'),
        ('duration', '-0.001', '(?i)duration'),
        ('duration', '0.0001', 'millisecond precision'),
        ('min_ttl', '256', 'must be'),
        ('max_ttl', '-1', 'must be'),
        ('min_ttl', '65', 'min_ttl'),
    ],
)
def test_observation_field_validation_matches_mapping_and_indexed_adapters(
    tmp_path: Path,
    adapter: str,
    column: str,
    value: str,
    message: str,
) -> None:
    csv_ingest, normalized_rows = load_modules()
    config_path = tmp_path / 'mapping.json'
    fields = ['te', 'src', 'dst', 'sp', 'dp', 'duration', 'min_ttl', 'max_ttl']
    config_path.write_text(
        """
        {
          "has_header": false,
          "fieldnames": ["te", "src", "dst", "sp", "dp", "duration", "min_ttl", "max_ttl"],
          "columns": {
            "time_end": "te",
            "src_ip": "src",
            "dst_ip": "dst",
            "src_port": "sp",
            "dst_port": "dp",
            "duration": "duration",
            "min_ttl": "min_ttl",
            "max_ttl": "max_ttl"
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
        'sp': '1',
        'dp': '2',
        'duration': '1.234',
        'min_ttl': '31',
        'max_ttl': '64',
    }
    row[column] = value

    with pytest.raises(csv_ingest.CsvSourceConfigError, match=message):
        if adapter == 'mapping':
            normalized_rows.normalize_csv_row(row, config)
        else:
            normalized_rows.normalize_csv_values(
                [row[field] for field in fields],
                config,
                {field: index for index, field in enumerate(fields)},
            )
