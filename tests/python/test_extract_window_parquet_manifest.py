import json
from argparse import Namespace
from datetime import datetime
from pathlib import Path

import pytest

from extract_window_helpers import (
    PORTABLE_TABLES,
    SQLITE_SIDECAR_SUFFIXES,
    extract_args,
    load_module,
    make_source_db,
)


def test_extract_repeated_outputs_write_sqlite_and_parquet(tmp_path: Path) -> None:
    pytest.importorskip('pyarrow')
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    manifest_path = module.extract(
        extract_args(module, source_path, output_dir, '--output', 'sqlite', '--output', 'parquet')
    )
    manifest = json.loads(manifest_path.read_text())

    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / 'parquet').is_dir()
    assert sorted(path.stem for path in (output_dir / 'parquet').glob('*.parquet')) == sorted(
        PORTABLE_TABLES
    )
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] == str(output_dir / 'parquet')


def test_extract_leaves_previously_enabled_outputs_when_disabled(tmp_path: Path) -> None:
    pytest.importorskip('pyarrow')
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    module.extract(
        extract_args(module, source_path, output_dir, '--output', 'sqlite', '--output', 'parquet')
    )
    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / 'parquet' / 'traffic_stats.parquet').exists()

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())

    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / 'parquet' / 'traffic_stats.parquet').exists()
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] is None

    manifest_path = module.extract(extract_args(module, source_path, output_dir, '--output', 'parquet'))
    manifest = json.loads(manifest_path.read_text())

    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / 'parquet' / 'traffic_stats.parquet').exists()
    assert manifest['outputs']['sqlite_path'] is None
    assert manifest['outputs']['parquet_dir'] == str(output_dir / 'parquet')


def test_extract_leaves_previous_custom_parquet_dir_when_disabled(tmp_path: Path) -> None:
    pytest.importorskip('pyarrow')
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    custom_parquet_dir = tmp_path / 'custom-parquet'

    module.extract(
        extract_args(
            module,
            source_path,
            output_dir,
            '--output',
            'parquet',
            '--parquet-dir',
            str(custom_parquet_dir),
        )
    )
    assert (custom_parquet_dir / 'traffic_stats.parquet').exists()

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())

    assert (custom_parquet_dir / 'traffic_stats.parquet').exists()
    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] is None


def test_extract_ignores_previous_custom_parquet_dir_when_disabled(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    custom_parquet_dir = tmp_path / 'custom-parquet'
    custom_parquet_dir.mkdir()
    sentinel = custom_parquet_dir / 'keep.txt'
    sentinel.write_text('not managed\n', encoding='utf-8')
    output_dir.mkdir()
    (output_dir / module.MANIFEST_FILENAME).write_text(
        json.dumps({'outputs': {'parquet_dir': str(custom_parquet_dir)}}),
        encoding='utf-8',
    )

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())

    assert custom_parquet_dir.is_dir()
    assert sentinel.read_text(encoding='utf-8') == 'not managed\n'
    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] is None


def test_extract_keeps_existing_default_parquet_dir_when_disabled(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    parquet_dir = output_dir / 'parquet'
    parquet_dir.mkdir(parents=True)
    sentinel = parquet_dir / 'keep.txt'
    sentinel.write_text('not managed\n', encoding='utf-8')

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())

    assert parquet_dir.is_dir()
    assert sentinel.read_text(encoding='utf-8') == 'not managed\n'
    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] is None


def test_extract_allows_source_db_inside_disabled_parquet_dir(tmp_path: Path) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    source_path = output_dir / 'parquet' / 'source.sqlite'
    source_path.parent.mkdir(parents=True)
    make_source_db(source_path).close()

    manifest_path = module.extract(extract_args(module, source_path, output_dir))
    manifest = json.loads(manifest_path.read_text())

    assert source_path.exists()
    assert (output_dir / module.SQLITE_FILENAME).exists()
    assert manifest['outputs']['parquet_dir'] is None


def test_output_modes_rejects_programmatic_singular_output_alias() -> None:
    module = load_module()

    with pytest.raises(SystemExit, match='outputs'):
        module.output_modes(Namespace(output='parquet', outputs=None))


@pytest.mark.parametrize('managed_filename', ['netflow.sqlite', 'manifest.json'])
def test_extract_rejects_parquet_dir_overlapping_managed_outputs(
    tmp_path: Path, managed_filename: str
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    with pytest.raises(SystemExit):
        module.extract(
            extract_args(
                module,
                source_path,
                output_dir,
                '--output',
                'parquet',
                '--parquet-dir',
                str(output_dir / managed_filename),
            )
        )


@pytest.mark.parametrize('sidecar_suffix', SQLITE_SIDECAR_SUFFIXES)
def test_extract_rejects_parquet_dir_at_managed_sqlite_sidecar_before_snapshot(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    sidecar_suffix: str,
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    parquet_dir = output_dir / f'{module.SQLITE_FILENAME}{sidecar_suffix}'

    def fail_if_rejection_is_missed(source_db: Path, snapshot_path: Path) -> None:
        raise AssertionError(
            f'extraction reached source snapshot before rejecting parquet dir {parquet_dir}'
        )

    monkeypatch.setattr(module, 'create_source_snapshot', fail_if_rejection_is_missed)

    with pytest.raises(SystemExit):
        module.extract(
            extract_args(
                module,
                source_path,
                output_dir,
                '--output',
                'parquet',
                '--parquet-dir',
                str(parquet_dir),
            )
        )


@pytest.mark.parametrize('sidecar_suffix', SQLITE_SIDECAR_SUFFIXES)
def test_extract_rejects_source_db_at_managed_sqlite_sidecar_before_snapshot(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    sidecar_suffix: str,
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    output_dir.mkdir()
    source_path = output_dir / f'{module.SQLITE_FILENAME}{sidecar_suffix}'
    make_source_db(source_path).close()

    def fail_if_rejection_is_missed(source_db: Path, snapshot_path: Path) -> None:
        raise AssertionError(
            f'extraction reached source snapshot before rejecting source DB {source_db}'
        )

    monkeypatch.setattr(module, 'create_source_snapshot', fail_if_rejection_is_missed)

    with pytest.raises(SystemExit):
        module.extract(extract_args(module, source_path, output_dir))


@pytest.mark.parametrize(
    ('case_name', 'source_relative_path', 'parquet_relative_path'),
    [
        ('source_inside_parquet_dir', Path('parquet/source.sqlite'), Path('parquet')),
        ('parquet_dir_is_source_db', Path('source.sqlite'), Path('source.sqlite')),
    ],
)
def test_extract_rejects_parquet_dir_overlapping_source_db(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    case_name: str,
    source_relative_path: Path,
    parquet_relative_path: Path,
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    source_path = tmp_path / source_relative_path
    source_path.parent.mkdir(parents=True, exist_ok=True)
    make_source_db(source_path).close()

    def fail_if_rejection_is_missed(source_db: Path, snapshot_path: Path) -> None:
        raise AssertionError(
            f'{case_name}: extraction reached source snapshot before rejecting {source_db}'
        )

    monkeypatch.setattr(module, 'create_source_snapshot', fail_if_rejection_is_missed)

    with pytest.raises(SystemExit, match='--parquet-dir.*source DB|source DB.*--parquet-dir'):
        module.extract(
            extract_args(
                module,
                source_path,
                output_dir,
                '--output',
                'parquet',
                '--parquet-dir',
                str(tmp_path / parquet_relative_path),
            )
        )


def test_extract_output_parquet_uses_default_parquet_dir_under_output_dir(tmp_path: Path) -> None:
    pytest.importorskip('pyarrow')
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    manifest_path = module.extract(extract_args(module, source_path, output_dir, '--output', 'parquet'))
    manifest = json.loads(manifest_path.read_text())

    assert manifest['outputs']['parquet_dir'] == str(output_dir / 'parquet')
    assert (output_dir / 'parquet').is_dir()
    assert (output_dir / 'parquet' / 'traffic_stats.parquet').exists()


@pytest.mark.parametrize('custom_target', [False, True])
def test_extract_replaces_existing_parquet_dir_when_parquet_output_requested(
    tmp_path: Path,
    custom_target: bool,
) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    parquet_dir = tmp_path / 'custom-parquet' if custom_target else output_dir / 'parquet'
    parquet_dir.mkdir(parents=True)
    sentinel = parquet_dir / 'keep.txt'
    sentinel.write_text('not managed\n', encoding='utf-8')
    extra_args = ['--output', 'parquet']
    if custom_target:
        extra_args.extend(['--parquet-dir', str(parquet_dir)])

    module.extract(extract_args(module, source_path, output_dir, *extra_args))

    assert not sentinel.exists()
    assert (parquet_dir / 'traffic_stats.parquet').exists()
    assert not (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / module.MANIFEST_FILENAME).exists()


def test_extract_replaces_empty_existing_parquet_dir_when_parquet_output_requested(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    parquet_dir = output_dir / 'parquet'
    parquet_dir.mkdir(parents=True)

    module.extract(extract_args(module, source_path, output_dir, '--output', 'parquet'))

    assert (parquet_dir / 'traffic_stats.parquet').exists()
    assert not (output_dir / module.SQLITE_FILENAME).exists()
    assert (output_dir / module.MANIFEST_FILENAME).exists()


def test_export_parquet_keeps_typed_schema_after_initial_null_batch(tmp_path: Path) -> None:
    pa = pytest.importorskip('pyarrow')
    pq = pytest.importorskip('pyarrow.parquet')
    module = load_module()
    source_conn = module.connect_db(tmp_path / 'source.sqlite')
    source_conn.execute(
        'CREATE TABLE nullable_values ('
        'bucket_start INTEGER NOT NULL, source_id TEXT NOT NULL, '
        'observed_count INTEGER, note TEXT)'
    )
    source_conn.executemany(
        'INSERT INTO nullable_values VALUES (?, ?, ?, ?)',
        [
            (100, 'r1', None, None),
            (200, 'r1', 42, 'typed'),
        ],
    )
    source_conn.commit()

    output_path = tmp_path / 'nullable_values.parquet'
    written = module.export_table_to_parquet(
        source_conn, output_path, 'nullable_values', 'ORDER BY bucket_start', (), 1
    )
    table = pq.read_table(output_path)

    assert written == 2
    assert table.column('observed_count').to_pylist() == [None, 42]
    assert table.column('note').to_pylist() == [None, 'typed']
    assert pa.types.is_integer(table.schema.field('observed_count').type)
    assert pa.types.is_string(table.schema.field('note').type)


def test_export_empty_parquet_preserves_usable_schema(tmp_path: Path) -> None:
    pa = pytest.importorskip('pyarrow')
    pq = pytest.importorskip('pyarrow.parquet')
    module = load_module()
    source_conn = module.connect_db(tmp_path / 'source.sqlite')
    source_conn.execute(
        'CREATE TABLE empty_metrics ('
        'bucket_start INTEGER NOT NULL, source_id TEXT NOT NULL, ratio REAL)'
    )
    source_conn.commit()

    output_path = tmp_path / 'empty_metrics.parquet'
    written = module.export_table_to_parquet(
        source_conn, output_path, 'empty_metrics', 'WHERE 0', (), 10
    )
    table = pq.read_table(output_path)

    assert written == 0
    assert table.schema.names == ['bucket_start', 'source_id', 'ratio']
    assert pa.types.is_integer(table.schema.field('bucket_start').type)
    assert pa.types.is_string(table.schema.field('source_id').type)
    assert pa.types.is_floating(table.schema.field('ratio').type)


def test_manifest_contains_portable_extract_contract(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'

    manifest_path = module.extract(
        extract_args(
            module,
            source_path,
            output_dir,
            '--dataset',
            'uoregon',
            '--source-id',
            'r1',
            '--granularity',
            '5m',
        )
    )
    written = manifest_path.read_text(encoding='utf-8')
    manifest = json.loads(written)

    assert written.endswith('\n')
    assert {'dataset_id', 'generated_at', 'filters', 'window', 'outputs', 'tables'} <= set(manifest)
    assert manifest['dataset_id'] == 'uoregon'
    assert datetime.fromisoformat(manifest['generated_at'])
    assert manifest['filters'] == {'source_id': 'r1', 'granularities': ['5m']}
    assert manifest['start_input'] == '150'
    assert manifest['end_exclusive_input'] == '500'
    assert datetime.fromisoformat(manifest['start'])
    assert datetime.fromisoformat(manifest['end_exclusive'])
    assert manifest['window']['start_input'] == '150'
    assert manifest['window']['end_exclusive_input'] == '500'
    assert manifest['window']['start'] == manifest['start']
    assert manifest['window']['end_exclusive'] == manifest['end_exclusive']
    assert manifest['window']['start_ts'] == 150
    assert manifest['window']['end_exclusive_ts'] == 500
    assert 'end' not in manifest
    assert 'end' not in manifest['window']
    assert manifest['outputs']['output_dir'] == str(output_dir)
    assert manifest['outputs']['sqlite_path'] == str(output_dir / module.SQLITE_FILENAME)
    assert manifest['outputs']['parquet_dir'] is None
    assert set(manifest['tables']) == PORTABLE_TABLES
    assert manifest['tables']['traffic_stats']['sqlite_row_count'] == 1
    assert manifest['tables']['traffic_stats']['source_row_count'] == 1
