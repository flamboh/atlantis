import importlib
import io
import json
import tarfile

import pytest


def write_mapping(
    tmp_path,
    *,
    has_header: bool = False,
    skip_bad_column_count: bool = False,
    input_order: str = 'timestamp_ascending',
    delimiter: str = ',',
    out_of_order_lag_buckets: int = 12,
    payload_overrides: dict | None = None,
):
    fields = ['te', 'src', 'dst', 'proto', 'packets', 'bytes', 'stos']
    payload = {
        'has_header': has_header,
        'timestamp_format': 'datetime',
        'timestamp_timezone': 'UTC',
        'columns': {
            'time_end': 'te',
            'src_ip': 'src',
            'dst_ip': 'dst',
            'protocol': 'proto',
            'packets': 'packets',
            'bytes': 'bytes',
            'src_tos': 'stos',
        },
        'source_id': {'value': 'r1'},
        'delimiter': delimiter,
        'skip_bad_column_count': skip_bad_column_count,
        'out_of_order_lag_buckets': out_of_order_lag_buckets,
        'input_order': input_order,
    }
    if not has_header:
        payload['fieldnames'] = fields
    if payload_overrides:
        payload.update(payload_overrides)
    path = tmp_path / f'mapping-{has_header}.json'
    path.write_text(json.dumps(payload), encoding='utf-8')
    return path


def scan(module, csv_path, mapping_path):
    return list(module.scan_csv({'path': str(csv_path), 'mapping_path': str(mapping_path)}))


@pytest.mark.parametrize('has_header', [False, True])
def test_scan_interface_normalizes_adapters_identically(tmp_path, has_header: bool) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path, has_header=has_header)
    data = '2016-07-27 13:43:30, 192.0.2.1 , 198.51.100.1 , tCp , 1 , 2 , 3 \n'
    if has_header:
        data = 'te,src,dst,proto,packets,bytes,stos\n' + data
    csv_path = tmp_path / f'flows-{has_header}.csv'
    csv_path.write_text(data, encoding='utf-8')

    events = scan(module, csv_path, mapping)

    ready = events[0]
    traffic = next(entry for entry in ready.bucket.traffic if entry.scope.src_visibility == 'all')
    assert (traffic.metrics.flows_tcp, traffic.metrics.packets, traffic.metrics.bytes) == (1, 1, 2)
    assert events[-1].rejected_rows == 0


def test_generic_indexed_and_arrow_adapters_emit_identical_rich_observations(
    tmp_path,
    monkeypatch,
) -> None:
    module = importlib.import_module('csv_scan')
    fields = [
        'tr',
        'te',
        'src',
        'dst',
        'sp',
        'dp',
        'proto',
        'packets',
        'bytes',
        'stos',
        'dtos',
        'duration',
        'min_ttl',
        'max_ttl',
    ]
    columns = {
        'time_received': 'tr',
        'time_end': 'te',
        'src_ip': 'src',
        'dst_ip': 'dst',
        'src_port': 'sp',
        'dst_port': 'dp',
        'protocol': 'proto',
        'packets': 'packets',
        'bytes': 'bytes',
        'src_tos': 'stos',
        'dst_tos': 'dtos',
        'duration': 'duration',
        'min_ttl': 'min_ttl',
        'max_ttl': 'max_ttl',
    }
    values = [
        '2016-07-27 13:43:30',
        '2016-07-27 13:43:29',
        '192.0.2.1',
        '198.51.100.1',
        '0',
        '443',
        'TCP',
        '10',
        '2048',
        '2',
        '3',
        '1.234',
        '31',
        '64',
    ]
    accepted = []
    monkeypatch.setattr(module._ScanState, 'accept', lambda _self, row: accepted.append(row))

    observations = []
    for name, has_header, delimiter in (
        ('generic', True, ','),
        ('indexed', False, '|'),
        ('arrow', False, ','),
    ):
        mapping = write_mapping(
            tmp_path,
            has_header=has_header,
            delimiter=delimiter,
            payload_overrides={'fieldnames': fields, 'columns': columns},
        )
        data = delimiter.join(values) + '\n'
        if has_header:
            data = delimiter.join(fields) + '\n' + data
        csv_path = tmp_path / f'{name}.csv'
        csv_path.write_text(data, encoding='utf-8')
        before = len(accepted)

        events = scan(module, csv_path, mapping)

        assert events[-1].rejected_rows == 0
        assert len(accepted) == before + 1
        observations.append(accepted[-1])

    assert observations[0] == observations[1] == observations[2]
    observation = observations[0].observation
    assert observation.time_received_ms == 1469627010000
    assert observation.time_end_ms == 1469627009000
    assert observation.src_port == 0
    assert observation.dst_port == 443
    assert observation.duration_ms == 1234
    assert (observation.min_ttl, observation.max_ttl) == (31, 64)


def test_rejected_trailing_row_establishes_zero_coverage_and_display_end(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path)
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n'
        '2016-07-27 13:50:00,not-an-ip,198.51.100.1,6,1,2,0\n',
        encoding='utf-8',
    )

    events = scan(module, csv_path, mapping)
    buckets = [event for event in events if isinstance(event, module.CsvBucketReady)]

    assert len(buckets) == 3
    assert [bucket.bucket.key.bucket_start for bucket in buckets] == [1469626800, 1469627100, 1469627400]
    assert buckets[-1].input_locator.endswith('/r1/1469627400')
    assert all(entry.metrics.flows == 0 for entry in buckets[-1].bucket.traffic)
    assert events[-1].rejected_rows == 1
    assert events[-1].observed_bounds['r1'][1] == buckets[-1].bucket.key.bucket_start


def test_fatal_column_shape_does_not_emit_completion(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path)
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text('2016-07-27 13:40:00,192.0.2.1\n', encoding='utf-8')
    events = module.scan_csv({'path': str(csv_path), 'mapping_path': str(mapping)})

    with pytest.raises(module.CsvSourceConfigError, match='Expected 7 columns'):
        list(events)


def test_archive_members_share_scan_coverage_and_preserve_owner(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path)
    archive_path = tmp_path / 'flows.tar.gz'
    with tarfile.open(archive_path, 'w:gz') as archive:
        for name, row in (
            ('a.csv', '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n'),
            ('b.csv', '2016-07-27 13:50:00,192.0.2.2,198.51.100.2,17,1,2,0\n'),
        ):
            payload = row.encode()
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))

    events = scan(module, archive_path, mapping)
    buckets = [event for event in events if isinstance(event, module.CsvBucketReady)]

    assert len(buckets) == 3
    assert all(bucket.scan_locator == str(archive_path) for bucket in buckets)
    assert buckets[1].input_locator.endswith('/r1/1469627100')


def test_unsorted_scan_emits_ordered_dense_buckets(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path, input_order='unsorted')
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '2016-07-27 13:50:00,192.0.2.2,198.51.100.2,17,1,2,0\n'
        '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n',
        encoding='utf-8',
    )

    events = scan(module, csv_path, mapping)
    buckets = [event.bucket for event in events if isinstance(event, module.CsvBucketReady)]

    assert [bucket.key.bucket_start for bucket in buckets] == [1469626800, 1469627100, 1469627400]


def test_skip_bad_column_count_is_counted_without_rejecting_content(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path, skip_bad_column_count=True)
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text(
        '2016-07-27 13:40:00,too,few\n'
        '2016-07-27 13:45:00,192.0.2.1,198.51.100.1,6,1,2,0\n',
        encoding='utf-8',
    )

    events = scan(module, csv_path, mapping)
    completion = events[-1]

    assert completion.skipped_bad_column_count == 1
    assert completion.rejected_rows == 0


def test_gap_locator_is_stable_and_unique_to_the_owning_scan() -> None:
    module = importlib.import_module('csv_scan')

    first = module.csv_gap_locator('/csv/first file.csv', 'r/1', 300)
    repeated = module.csv_gap_locator('/csv/first file.csv', 'r/1', 300)
    second = module.csv_gap_locator('/csv/second.csv', 'r/1', 300)

    assert first == repeated
    assert first != second
    assert first == 'gap://csv/%2Fcsv%2Ffirst%20file.csv/r%2F1/300'


def test_gap_locator_includes_scan_identity() -> None:
    module = importlib.import_module('csv_scan')

    first = module.csv_gap_locator('/feeds/one.csv', 'router/1', 300)
    second = module.csv_gap_locator('/feeds/two.csv', 'router/1', 300)

    assert first != second
    assert first == 'gap://csv/%2Ffeeds%2Fone.csv/router%2F1/300'


def test_headerless_non_arrow_scan_uses_indexed_normalizer(tmp_path, monkeypatch) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(tmp_path, delimiter='|')
    csv_path = tmp_path / 'indexed.csv'
    csv_path.write_text(
        '2016-07-27 13:43:30|192.0.2.1|198.51.100.1|6|1|2|3\n',
        encoding='utf-8',
    )
    calls = 0
    original = module.normalize_csv_values
    config = module.load_csv_source_config(mapping)
    state = module._ScanState(str(csv_path), config)
    raw = next(module._iter_raw_rows(csv_path, config, state))

    assert raw.values is None
    assert raw.indexed_values == [
        '2016-07-27 13:43:30',
        '192.0.2.1',
        '198.51.100.1',
        '6',
        '1',
        '2',
        '3',
    ]

    def track(values, config, indexes):
        nonlocal calls
        calls += 1
        return original(values, config, indexes)

    monkeypatch.setattr(module, 'normalize_csv_values', track)
    monkeypatch.setattr(
        module._ScanState,
        'observe',
        lambda *_args: pytest.fail('indexed coverage used the mapping observer'),
    )
    monkeypatch.setattr(
        module,
        'normalize_csv_row',
        lambda *_args: pytest.fail('headerless adapter reconstructed a dictionary'),
    )

    events = scan(module, csv_path, mapping)

    assert calls == 1
    assert events[-1].rejected_rows == 0


@pytest.mark.parametrize('timestamp_format', ['unix', 'unix_ms'])
@pytest.mark.parametrize('timestamp', ['nan', 'inf', '1e999'])
def test_indexed_scan_recovers_from_invalid_numeric_timestamp(
    tmp_path,
    timestamp_format: str,
    timestamp: str,
) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(
        tmp_path,
        delimiter='|',
        payload_overrides={'timestamp_format': timestamp_format},
    )
    csv_path = tmp_path / 'numeric.csv'
    csv_path.write_text(
        f'{timestamp}|192.0.2.1|198.51.100.1|6|1|2|3\n',
        encoding='utf-8',
    )

    events = scan(module, csv_path, mapping)

    assert events[-1].rejected_rows == 1
    assert events[-1].observed_bounds == {}


def test_arrow_rejects_late_row_from_later_record_batch(tmp_path, monkeypatch) -> None:
    module = importlib.import_module('csv_scan')
    monkeypatch.setattr(module, 'CSV_ARROW_BLOCK_BYTES', 256)
    mapping = write_mapping(tmp_path, out_of_order_lag_buckets=0)
    csv_path = tmp_path / 'late.csv'
    early = '2016-07-27 13:40:00,192.0.2.1,198.51.100.1,6,1,2,0\n'
    current = '2016-07-27 13:50:00,192.0.2.2,198.51.100.2,17,1,2,0\n'
    csv_path.write_text(early + (current * 20) + early, encoding='utf-8')

    with pytest.raises(ValueError, match='not ordered enough for streaming'):
        scan(module, csv_path, mapping)


def test_arrow_rejects_invalid_secondary_timestamp_and_custom_protocol(tmp_path) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(
        tmp_path,
        payload_overrides={
            'fieldnames': ['tr', 'te', 'src', 'dst', 'proto', 'packets', 'bytes', 'stos'],
            'columns': {
                'time_received': 'tr',
                'time_end': 'te',
                'src_ip': 'src',
                'dst_ip': 'dst',
                'protocol': 'proto',
                'packets': 'packets',
                'bytes': 'bytes',
                'src_tos': 'stos',
            },
            'protocol_map': {'CUSTOM': 300},
        },
    )
    csv_path = tmp_path / 'invalid-arrow.csv'
    csv_path.write_text(
        '2016-07-27 13:40:00,not-a-time,192.0.2.1,198.51.100.1,6,1,2,0\n'
        '2016-07-27 13:45:00,2016-07-27 13:45:00,192.0.2.1,198.51.100.1,CUSTOM,1,2,0\n',
        encoding='utf-8',
    )

    events = scan(module, csv_path, mapping)
    buckets = [event for event in events if isinstance(event, module.CsvBucketReady)]

    assert events[-1].rejected_rows == 2
    assert [bucket.bucket.key.bucket_start for bucket in buckets] == [1469626800, 1469627100]
    assert all(entry.metrics.flows == 0 for bucket in buckets for entry in bucket.bucket.traffic)


@pytest.mark.parametrize(('column', 'value'), [('dtos', '256'), ('packets', str(1 << 63))])
def test_arrow_rejects_values_that_sqlite_or_generic_adapter_cannot_store(
    tmp_path,
    column: str,
    value: str,
) -> None:
    module = importlib.import_module('csv_scan')
    mapping = write_mapping(
        tmp_path,
        payload_overrides={
            'fieldnames': ['te', 'src', 'dst', 'proto', 'packets', 'bytes', 'stos', 'dtos'],
            'columns': {
                'time_end': 'te',
                'src_ip': 'src',
                'dst_ip': 'dst',
                'protocol': 'proto',
                'packets': 'packets',
                'bytes': 'bytes',
                'src_tos': 'stos',
                'dst_tos': 'dtos',
            },
        },
    )
    row = {
        'te': '2016-07-27 13:40:00',
        'src': '192.0.2.1',
        'dst': '198.51.100.1',
        'proto': '6',
        'packets': '1',
        'bytes': '2',
        'stos': '0',
        'dtos': '0',
    }
    row[column] = value
    fieldnames = json.loads(mapping.read_text(encoding='utf-8'))['fieldnames']
    csv_path = tmp_path / 'range.csv'
    csv_path.write_text(','.join(row[name] for name in fieldnames) + '\n', encoding='utf-8')

    events = scan(module, csv_path, mapping)

    assert events[-1].rejected_rows == 1
