import sqlite3
from pathlib import Path

import pytest

from extract_window_helpers import (
    copied_rows,
    extract_args,
    load_module,
    make_source_db,
    write_stale_sqlite_sidecars,
)


def test_extract_sqlite_publish_neutralizes_stale_sidecars(tmp_path: Path) -> None:
    module = load_module()
    source_path = tmp_path / 'source.sqlite'
    make_source_db(source_path).close()
    output_dir = tmp_path / 'out'
    output_dir.mkdir()
    sqlite_path = output_dir / module.SQLITE_FILENAME

    with sqlite3.connect(sqlite_path) as existing_conn:
        existing_conn.execute('CREATE TABLE old_generation (value TEXT NOT NULL)')
        existing_conn.execute('INSERT INTO old_generation VALUES (?)', ('stale-main',))
    stale_sidecars = write_stale_sqlite_sidecars(sqlite_path, 'stale')

    module.extract(extract_args(module, source_path, output_dir))

    for sidecar_path, stale_bytes in stale_sidecars.items():
        if sidecar_path.exists():
            assert sidecar_path.read_bytes() != stale_bytes

    with module.connect_db(sqlite_path) as conn:
        assert copied_rows(conn) == [
            ('r1', '1h', 200, 40),
            ('r1', '5m', 200, 20),
            ('r2', '5m', 200, 30),
        ]


def test_publish_outputs_rolls_back_when_later_publish_step_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    work_dir = tmp_path / 'work'
    parquet_dir = output_dir / 'parquet'
    temp_parquet_dir = work_dir / 'parquet'
    output_dir.mkdir()
    work_dir.mkdir()
    parquet_dir.mkdir()
    temp_parquet_dir.mkdir()

    sqlite_path = output_dir / module.SQLITE_FILENAME
    temp_sqlite_path = work_dir / module.SQLITE_FILENAME
    manifest_path = output_dir / module.MANIFEST_FILENAME
    temp_manifest_path = work_dir / module.MANIFEST_FILENAME
    parquet_sentinel = parquet_dir / 'keep.parquet'

    sqlite_path.write_bytes(b'old-db')
    manifest_path.write_text('{"generation": "old"}\n', encoding='utf-8')
    parquet_sentinel.write_bytes(b'old-parquet')
    temp_sqlite_path.write_bytes(b'new-db')
    temp_manifest_path.write_text('{"generation": "new"}\n', encoding='utf-8')
    (temp_parquet_dir / 'new.parquet').write_bytes(b'new-parquet')

    def fail_after_sqlite_publish(source: Path, target: Path, backup_parent: Path) -> None:
        assert source == temp_parquet_dir
        assert target == parquet_dir
        assert backup_parent == work_dir
        assert sqlite_path.read_bytes() == b'new-db'
        raise OSError('simulated publish failure')

    monkeypatch.setitem(module.publish_outputs.__globals__, 'replace_path', fail_after_sqlite_publish)

    with pytest.raises(OSError, match='simulated publish failure'):
        module.publish_outputs(
            temp_sqlite_path=temp_sqlite_path,
            sqlite_output_path=sqlite_path,
            temp_parquet_dir=temp_parquet_dir,
            parquet_dir=parquet_dir,
            temp_manifest_path=temp_manifest_path,
            manifest_path=manifest_path,
            work_dir=work_dir,
        )

    assert sqlite_path.read_bytes() == b'old-db'
    assert manifest_path.read_text(encoding='utf-8') == '{"generation": "old"}\n'
    assert parquet_sentinel.read_bytes() == b'old-parquet'
    assert sorted(path.name for path in parquet_dir.iterdir()) == ['keep.parquet']


def test_publish_outputs_restores_old_parquet_when_manifest_publish_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    work_dir = tmp_path / 'work'
    parquet_dir = output_dir / 'parquet'
    temp_parquet_dir = work_dir / 'parquet'
    output_dir.mkdir()
    work_dir.mkdir()
    parquet_dir.mkdir()
    temp_parquet_dir.mkdir()

    manifest_path = output_dir / module.MANIFEST_FILENAME
    temp_manifest_path = work_dir / module.MANIFEST_FILENAME
    parquet_sentinel = parquet_dir / 'keep.parquet'

    manifest_path.write_text('{"generation": "old"}\n', encoding='utf-8')
    temp_manifest_path.write_text('{"generation": "new"}\n', encoding='utf-8')
    parquet_sentinel.write_bytes(b'old-parquet')
    (temp_parquet_dir / 'new.parquet').write_bytes(b'new-parquet')

    original_publish_path = module.publish_outputs.__globals__['publish_path']

    def fail_manifest_publish(source: Path, target: Path, backup_root: Path):
        assert backup_root == work_dir
        if source == temp_manifest_path:
            assert target == manifest_path
            assert (parquet_dir / 'new.parquet').read_bytes() == b'new-parquet'
            assert not parquet_sentinel.exists()
            raise OSError('simulated manifest publish failure')
        return original_publish_path(source, target, backup_root)

    monkeypatch.setitem(module.publish_outputs.__globals__, 'publish_path', fail_manifest_publish)

    with pytest.raises(OSError, match='simulated manifest publish failure'):
        module.publish_outputs(
            temp_sqlite_path=None,
            sqlite_output_path=None,
            temp_parquet_dir=temp_parquet_dir,
            parquet_dir=parquet_dir,
            temp_manifest_path=temp_manifest_path,
            manifest_path=manifest_path,
            work_dir=work_dir,
        )

    assert manifest_path.read_text(encoding='utf-8') == '{"generation": "old"}\n'
    assert parquet_sentinel.read_bytes() == b'old-parquet'
    assert not (parquet_dir / 'new.parquet').exists()
    assert sorted(path.name for path in parquet_dir.iterdir()) == ['keep.parquet']


def test_publish_path_removes_cross_device_staging_when_replace_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = load_module()
    source_dir = tmp_path / 'work' / 'parquet'
    target_dir = tmp_path / 'out' / 'parquet'
    source_dir.mkdir(parents=True)
    target_dir.parent.mkdir(parents=True)
    (source_dir / 'new.parquet').write_bytes(b'new')

    def prepare_staged_publish(source: Path, target: Path, work_dir: Path):
        staging_path = target.parent / f'.incoming-{target.name}-{work_dir.name}'
        module.copy_path(source, staging_path)
        return staging_path, staging_path

    monkeypatch.setattr(
        module.publish_path.__globals__['os'],
        'replace',
        lambda source, target: (_ for _ in ()).throw(OSError('simulated replace failure')),
    )
    monkeypatch.setitem(module.publish_path.__globals__, 'prepare_publish_source', prepare_staged_publish)

    with pytest.raises(OSError, match='simulated replace failure'):
        module.publish_path(source_dir, target_dir, tmp_path / 'work')

    assert not any(target_dir.parent.glob('.incoming-parquet-*'))


def test_prepare_publish_source_removes_cross_device_staging_when_copy_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = load_module()
    source_dir = tmp_path / 'work' / 'parquet'
    target_dir = tmp_path / 'out' / 'parquet'
    source_dir.mkdir(parents=True)
    target_dir.parent.mkdir(parents=True)
    (source_dir / 'new.parquet').write_bytes(b'new')

    class CrossDeviceSource:
        def stat(self):
            return type('Stat', (), {'st_dev': -1})()

    def fail_copy(source: Path, target: Path) -> None:
        target.mkdir(parents=True)
        (target / 'partial.parquet').write_bytes(b'partial')
        raise OSError('simulated copy failure')

    monkeypatch.setitem(module.prepare_publish_source.__globals__, 'copy_path', fail_copy)

    with pytest.raises(OSError, match='simulated copy failure'):
        module.prepare_publish_source(CrossDeviceSource(), target_dir, tmp_path / 'work')

    assert not any(target_dir.parent.glob('.incoming-parquet-*'))


def test_publish_outputs_restores_sqlite_sidecars_when_later_publish_step_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = load_module()
    output_dir = tmp_path / 'out'
    work_dir = tmp_path / 'work'
    parquet_dir = output_dir / 'parquet'
    temp_parquet_dir = work_dir / 'parquet'
    output_dir.mkdir()
    work_dir.mkdir()
    parquet_dir.mkdir()
    temp_parquet_dir.mkdir()

    sqlite_path = output_dir / module.SQLITE_FILENAME
    temp_sqlite_path = work_dir / module.SQLITE_FILENAME
    manifest_path = output_dir / module.MANIFEST_FILENAME
    temp_manifest_path = work_dir / module.MANIFEST_FILENAME
    sqlite_path.write_bytes(b'old-main-db')
    temp_sqlite_path.write_bytes(b'new-main-db')
    manifest_path.write_text('{"generation": "old"}\n', encoding='utf-8')
    temp_manifest_path.write_text('{"generation": "new"}\n', encoding='utf-8')
    (temp_parquet_dir / 'new.parquet').write_bytes(b'new-parquet')
    old_sidecars = write_stale_sqlite_sidecars(sqlite_path, 'old')
    observed_after_sqlite: dict[Path, bytes | None] = {}

    def fail_after_sqlite_publish(source: Path, target: Path, backup_parent: Path) -> None:
        assert source == temp_parquet_dir
        assert target == parquet_dir
        assert backup_parent == work_dir
        assert sqlite_path.read_bytes() == b'new-main-db'
        observed_after_sqlite.update(
            {
                sidecar_path: sidecar_path.read_bytes() if sidecar_path.exists() else None
                for sidecar_path in old_sidecars
            }
        )
        raise OSError('simulated publish failure')

    monkeypatch.setitem(module.publish_outputs.__globals__, 'replace_path', fail_after_sqlite_publish)

    with pytest.raises(OSError, match='simulated publish failure'):
        module.publish_outputs(
            temp_sqlite_path=temp_sqlite_path,
            sqlite_output_path=sqlite_path,
            temp_parquet_dir=temp_parquet_dir,
            parquet_dir=parquet_dir,
            temp_manifest_path=temp_manifest_path,
            manifest_path=manifest_path,
            work_dir=work_dir,
        )

    assert observed_after_sqlite
    for sidecar_path, old_bytes in old_sidecars.items():
        assert observed_after_sqlite[sidecar_path] != old_bytes
        assert sidecar_path.read_bytes() == old_bytes
    assert sqlite_path.read_bytes() == b'old-main-db'
    assert manifest_path.read_text(encoding='utf-8') == '{"generation": "old"}\n'
