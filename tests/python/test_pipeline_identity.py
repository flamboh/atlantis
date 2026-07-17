import importlib
import sqlite3

import pytest


def load_modules():
    product = importlib.reload(importlib.import_module('pipeline_product'))
    revision = importlib.reload(importlib.import_module('input_revision'))
    stats = importlib.reload(importlib.import_module('stats'))
    return product, revision, stats


def identity(product, *, selection=None, config=None):
    return product.ProductIdentity.create(
        schema={'version': 1, 'tables': ['traffic_stats']},
        selection=selection or {'version': 1, 'kind': 'all'},
        config=config or {'version': 1, 'timezone': 'UTC'},
    )


def test_product_identity_binds_empty_database_and_reports_component_mismatch() -> None:
    product, _revision, stats = load_modules()
    conn = sqlite3.connect(':memory:')
    stats.init_stats_tables(conn)
    expected = identity(product)

    product.bind_product_identity(
        conn,
        expected,
        output_table_names=stats.STATS_TABLE_NAMES,
    )
    product.bind_product_identity(
        conn,
        expected,
        output_table_names=stats.STATS_TABLE_NAMES,
    )

    with pytest.raises(product.ProductIdentityConflict, match='selection'):
        product.bind_product_identity(
            conn,
            identity(product, selection={'version': 1, 'kind': 'none'}),
            output_table_names=stats.STATS_TABLE_NAMES,
        )


def test_pipeline_product_identity_uses_normalized_flow_selection() -> None:
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    flow_selection = importlib.import_module('flow_selection')
    pipeline_product = importlib.import_module('pipeline_product')
    conn = sqlite3.connect(':memory:')
    selected = flow_selection.FlowSelection.from_payload({'ip_prefix': '192.0.2.99/24'})

    pipeline.init_stats_tables(conn)
    pipeline.bind_current_product(
        conn,
        run_maad=False,
        maad_backend='python',
        selection=selected,
    )

    stored = conn.execute(
        'SELECT selection_json FROM pipeline_product WHERE singleton = 1'
    ).fetchone()[0]
    assert '192.0.2.0/24' in stored
    with pytest.raises(pipeline_product.ProductIdentityConflict, match='selection'):
        pipeline.bind_current_product(
            conn,
            run_maad=False,
            maad_backend='python',
            selection=flow_selection.FlowSelection.from_payload(
                {'ip_prefix': '198.51.100.0/24'}
            ),
        )


def test_product_identity_rejects_populated_legacy_database() -> None:
    product, _revision, stats = load_modules()
    conn = sqlite3.connect(':memory:')
    stats.init_stats_tables(conn)
    conn.execute(
        """
        INSERT INTO traffic_stats (
            source_id, granularity, bucket_start, bucket_end, ip_version,
            src_visibility, dst_visibility,
            flows, flows_tcp, flows_udp, flows_icmp, flows_other,
            packets, packets_tcp, packets_udp, packets_icmp, packets_other,
            bytes, bytes_tcp, bytes_udp, bytes_icmp, bytes_other
        ) VALUES ('r1', '5m', 0, 300, 4, 'all', 'all',
                  0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        """
    )

    with pytest.raises(product.ProductIdentityConflict, match='populated legacy'):
        product.bind_product_identity(
            conn,
            identity(product),
            output_table_names=stats.STATS_TABLE_NAMES,
        )


def test_input_revision_is_exact_and_decoder_fingerprint_is_canonical(tmp_path) -> None:
    _product, revision, _stats = load_modules()
    csv_ingest = importlib.reload(importlib.import_module('csv_ingest'))
    mapping_a = tmp_path / 'a.json'
    mapping_b = tmp_path / 'b.json'
    mapping_a.write_text(
        '{"columns":{"time_end":"te","src_ip":"src","dst_ip":"dst"},'
        '"source_id":{"value":"r1"}}',
        encoding='utf-8',
    )
    mapping_b.write_text(
        '{ "source_id": { "value": "r1" }, "columns": '
        '{ "dst_ip": "dst", "src_ip": "src", "time_end": "te" } }',
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text('one', encoding='utf-8')
    config_a = csv_ingest.load_csv_source_config(mapping_a)
    config_b = csv_ingest.load_csv_source_config(mapping_b)

    first = revision.csv_input_revision(csv_path, config_a)
    assert revision.csv_decoder_fingerprint(config_a) == revision.csv_decoder_fingerprint(config_b)
    csv_path.write_text('two', encoding='utf-8')
    second = revision.csv_input_revision(csv_path, config_a)

    assert first.content_fingerprint != second.content_fingerprint
    assert first.fingerprint != second.fingerprint


def test_csv_pipeline_loads_shared_mapping_once_and_rejects_replaced_content(
    tmp_path,
    monkeypatch,
) -> None:
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    mapping = tmp_path / 'mapping.json'
    mapping.write_text(
        '{"has_header":true,"columns":{"time_end":"te","src_ip":"src",'
        '"dst_ip":"dst"},"source_id":{"value":"r1"}}',
        encoding='utf-8',
    )
    first = tmp_path / 'a.csv'
    second = tmp_path / 'b.csv'
    for path in (first, second):
        path.write_text('te,src,dst\n', encoding='utf-8')
    real_loader = pipeline.load_csv_source_config
    calls = []

    def load_once(path):
        calls.append(str(path))
        return real_loader(path)

    monkeypatch.setattr(pipeline, 'load_csv_source_config', load_once)
    conn = sqlite3.connect(':memory:')
    specs = [
        {'input_kind': 'csv', 'path': str(path), 'mapping_path': str(mapping)}
        for path in (first, second)
    ]
    pipeline.process_input_specs(
        conn,
        specs,
        maad_backend='python',
        run_maad=False,
    )

    assert calls == [str(mapping)]
    first.write_text(
        'te,src,dst\n2025-01-01 00:00:00,192.0.2.1,198.51.100.1\n',
        encoding='utf-8',
    )
    with pytest.raises(pipeline.InputRevisionConflict, match='content changed'):
        pipeline.process_input_specs(
            conn,
            [specs[0]],
            maad_backend='python',
            run_maad=False,
        )


def test_nfcapd_processedness_compares_locator_and_revision() -> None:
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    processed = importlib.reload(importlib.import_module('processed_inputs'))
    _product, revision, _stats = load_modules()
    conn = sqlite3.connect(':memory:')
    processed.init_processed_inputs_table(conn)
    current = revision.InputRevision.create(
        input_kind='nfcapd',
        locator='/captures/nfcapd.202501010000',
        content_fingerprint='first',
        decoder_fingerprint='decoder',
    )
    processed.upsert_input_bucket(
        conn,
        input_kind='nfcapd',
        input_locator=current.locator,
        source_id='r1',
        bucket_start=0,
        bucket_end=300,
        input_revision=current,
    )
    processed.mark_input_bucket_status(
        conn,
        input_kind='nfcapd',
        input_locator=current.locator,
        source_id='r1',
        bucket_start=0,
        status='processed',
        input_revision=current,
    )

    assert pipeline.nfcapd_logical_bucket_processed(conn, 'r1', 0, [current])
    replacement = revision.InputRevision.create(
        input_kind='nfcapd',
        locator=current.locator,
        content_fingerprint='second',
        decoder_fingerprint='decoder',
    )
    with pytest.raises(pipeline.InputRevisionConflict, match='force'):
        pipeline.nfcapd_logical_bucket_processed(conn, 'r1', 0, [replacement])


def test_completed_unchanged_input_reuses_digest_but_changed_snapshot_rehashes(
    tmp_path,
    monkeypatch,
) -> None:
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    processed = importlib.reload(importlib.import_module('processed_inputs'))
    mapping = tmp_path / 'mapping.json'
    mapping.write_text(
        '{"columns":{"time_end":"te","src_ip":"src","dst_ip":"dst"},'
        '"source_id":{"value":"r1"}}',
        encoding='utf-8',
    )
    csv_path = tmp_path / 'flows.csv'
    csv_path.write_text('first', encoding='utf-8')
    spec = {
        'input_kind': 'csv',
        'path': str(csv_path),
        'mapping_path': str(mapping),
    }
    conn = sqlite3.connect(':memory:')
    first = pipeline.prepare_input_specs(conn, [spec])[0]
    processed.complete_input_scan(
        conn,
        input_kind='csv',
        scan_locator=str(csv_path),
        input_revision=first['input_revision'],
        file_snapshot=first['_file_snapshot'],
        rejected_rows=0,
    )

    def unexpected_hash(*_args, **_kwargs):
        raise AssertionError('unchanged completed input should reuse its exact digest')

    monkeypatch.setattr(pipeline, 'capture_csv_input_revision', unexpected_hash)
    reused = pipeline.prepare_input_specs(conn, [spec])[0]
    assert reused['input_revision'] == first['input_revision']

    csv_path.write_text('second and different length', encoding='utf-8')
    calls = []
    real_capture = importlib.import_module('input_revision').capture_csv_input_revision

    def capture_changed(*args, **kwargs):
        calls.append(args[0])
        return real_capture(*args, **kwargs)

    monkeypatch.setattr(pipeline, 'capture_csv_input_revision', capture_changed)
    changed = pipeline.prepare_input_specs(conn, [spec])[0]
    assert calls == [str(csv_path)]
    with pytest.raises(processed.InputRevisionConflict, match='content changed'):
        pipeline.csv_input_fully_processed(conn, changed['input_revision'])


def test_nfcapd_tree_rejects_source_rename_and_reassignment_before_publication(
    tmp_path,
) -> None:
    pipeline = importlib.reload(importlib.import_module('pipeline'))
    product = importlib.reload(importlib.import_module('pipeline_product'))
    conn = sqlite3.connect(':memory:')
    root = tmp_path / 'captures'
    (root / 'member-a').mkdir(parents=True)
    (root / 'member-b').mkdir()

    def run(source_id: str, member_id: str = 'member-a') -> None:
        sources = [{'source_id': source_id, 'members': [member_id]}]
        pipeline.process_pipeline_config(
            conn,
            {
                'run_maad': False,
                'maad_backend': 'python',
                'datasets': [{'dataset_id': 'd1', 'sources': sources}],
                'inputs': [
                    {
                        'input_kind': 'nfcapd_tree',
                        'root_path': str(root),
                        'sources': sources,
                        'start_date': '2025-01-01',
                        'end_date': '2025-01-01',
                        'zero_fill_gaps': False,
                    }
                ],
            },
        )

    run('old-name')
    with pytest.raises(product.SourceLayoutConflict, match='membership changed'):
        run('old-name', 'member-b')
    with pytest.raises(product.SourceLayoutConflict, match='membership changed'):
        run('new-name')

    stored = conn.execute(
        'SELECT layout_json FROM nfcapd_source_layout WHERE singleton = 1'
    ).fetchone()[0]
    assert 'old-name' in stored
    assert 'new-name' not in stored
    assert conn.execute(
        'SELECT source_id, member_id FROM source_members WHERE dataset_id = ?',
        ('d1',),
    ).fetchall() == [('old-name', 'member-a')]
    assert conn.execute('SELECT COUNT(*) FROM processed_inputs').fetchone() == (0,)
