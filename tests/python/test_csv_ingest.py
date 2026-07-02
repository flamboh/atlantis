import importlib
import json
from pathlib import Path

import pytest


def load_module():
    module = importlib.import_module('csv_ingest')
    return importlib.reload(module)


def write_config(path: Path, payload) -> Path:
    path.write_text(json.dumps(payload), encoding='utf-8')
    return path


def test_load_csv_source_config_requires_object_root(tmp_path: Path) -> None:
    module = load_module()
    config_path = write_config(tmp_path / 'mapping.json', ['not', 'an', 'object'])

    with pytest.raises(module.CsvSourceConfigError, match='JSON object'):
        module.load_csv_source_config(config_path)


def test_load_csv_source_config_requires_timestamp_column(tmp_path: Path) -> None:
    module = load_module()
    config_path = write_config(
        tmp_path / 'mapping.json',
        {
            'columns': {
                'src_ip': 'src',
                'dst_ip': 'dst',
            },
            'source_id': {'value': 'uo-feed'},
        },
    )

    with pytest.raises(module.CsvSourceConfigError, match='timestamp'):
        module.load_csv_source_config(config_path)


def test_load_csv_source_config_requires_source_and_destination_ip_columns(tmp_path: Path) -> None:
    module = load_module()
    config_path = write_config(
        tmp_path / 'mapping.json',
        {
            'columns': {
                'time_received': 'received_at',
                'src_ip': 'src',
            },
            'source_id': {'value': 'uo-feed'},
        },
    )

    with pytest.raises(module.CsvSourceConfigError, match='dst_ip'):
        module.load_csv_source_config(config_path)


def test_load_csv_source_config_requires_source_id_value_or_column(tmp_path: Path) -> None:
    module = load_module()
    config_path = write_config(
        tmp_path / 'mapping.json',
        {
            'columns': {
                'time_received': 'received_at',
                'src_ip': 'src',
                'dst_ip': 'dst',
            },
        },
    )

    with pytest.raises(module.CsvSourceConfigError, match='source_id'):
        module.load_csv_source_config(config_path)


def test_load_csv_source_config_requires_fieldnames_without_header(tmp_path: Path) -> None:
    module = load_module()
    config_path = write_config(
        tmp_path / 'mapping.json',
        {
            'has_header': False,
            'columns': {
                'time_received': 'received_at',
                'src_ip': 'src',
                'dst_ip': 'dst',
            },
            'source_id': {'value': 'uo-feed'},
        },
    )

    with pytest.raises(module.CsvSourceConfigError, match='fieldnames'):
        module.load_csv_source_config(config_path)


def test_bucket_start_prefers_time_received_then_end_then_start(tmp_path: Path) -> None:
    module = load_module()
    config = module.load_csv_source_config(
        write_config(
            tmp_path / 'mapping.json',
            {
                'timestamp_format': 'unix',
                'columns': {
                    'time_received': 'received_at',
                    'time_end': 'ended_at',
                    'time_start': 'started_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'value': 'uo-feed'},
            },
        )
    )

    received_bucket = module.resolve_bucket_start(
        {
            'received_at': '1744733279',
            'ended_at': '1744733000',
            'started_at': '1744732700',
            'src': '192.0.2.10',
            'dst': '198.51.100.10',
        },
        config,
    )
    end_bucket = module.resolve_bucket_start(
        {
            'ended_at': '1744733000',
            'started_at': '1744732700',
            'src': '192.0.2.10',
            'dst': '198.51.100.10',
        },
        config,
    )
    start_bucket = module.resolve_bucket_start(
        {
            'started_at': '1744732700',
            'src': '192.0.2.10',
            'dst': '198.51.100.10',
        },
        config,
    )

    assert received_bucket == 1744733100
    assert end_bucket == 1744732800
    assert start_bucket == 1744732500


def test_parse_datetime_timestamp_uses_configured_timezone(tmp_path: Path) -> None:
    module = load_module()
    config = module.load_csv_source_config(
        write_config(
            tmp_path / 'mapping.json',
            {
                'timestamp_format': 'datetime',
                'timestamp_timezone': 'Europe/Madrid',
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'value': 'ugr16'},
            },
        )
    )

    bucket = module.resolve_bucket_start(
        {
            'te': '2016-07-27 13:43:30',
            'src': '42.219.154.107',
            'dst': '143.72.8.137',
        },
        config,
    )

    assert bucket == 1469619600


def test_parse_datetime_timestamp_uses_configured_format(tmp_path: Path) -> None:
    module = load_module()
    config = module.load_csv_source_config(
        write_config(
            tmp_path / 'mapping.json',
            {
                'timestamp_format': 'datetime',
                'datetime_format': '%Y/%m/%d %H:%M:%S',
                'timestamp_timezone': 'UTC',
                'columns': {
                    'time_end': 'te',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'value': 'feed'},
            },
        )
    )

    bucket = module.resolve_bucket_start(
        {
            'te': '2025/04/15 12:34:56',
            'src': '192.0.2.1',
            'dst': '198.51.100.1',
        },
        config,
    )

    assert bucket == 1744720200


def test_resolve_source_id_uses_constant_or_row_column(tmp_path: Path) -> None:
    module = load_module()
    constant_config = module.load_csv_source_config(
        write_config(
            tmp_path / 'constant.json',
            {
                'columns': {
                    'time_received': 'received_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'value': 'uo-feed'},
            },
        )
    )
    column_config = module.load_csv_source_config(
        write_config(
            tmp_path / 'column.json',
            {
                'columns': {
                    'time_received': 'received_at',
                    'src_ip': 'src',
                    'dst_ip': 'dst',
                },
                'source_id': {'column': 'router_name'},
            },
        )
    )

    assert (
        module.resolve_source_id(
            {'received_at': '1744733279', 'src': '192.0.2.1', 'dst': '198.51.100.1'},
            constant_config,
        )
        == 'uo-feed'
    )
    assert (
        module.resolve_source_id(
            {
                'received_at': '1744733279',
                'src': '192.0.2.1',
                'dst': '198.51.100.1',
                'router_name': ' oh_ir1_gw ',
            },
            column_config,
        )
        == 'oh_ir1_gw'
    )

    with pytest.raises(module.CsvSourceConfigError, match='source_id column'):
        module.resolve_source_id(
            {
                'received_at': '1744733279',
                'src': '192.0.2.1',
                'dst': '198.51.100.1',
                'router_name': '   ',
            },
            column_config,
        )
