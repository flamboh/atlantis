import importlib
import json
from datetime import datetime
from pathlib import Path

import pytest


def load_common():
    common = importlib.import_module('common')
    return importlib.reload(common)


def write_env(path: Path, datasets_path: Path) -> None:
    path.write_text(
        '\n'.join(
            [
                f'DATASETS_CONFIG_PATH={datasets_path}',
                'DEFAULT_DATASET=alpha',
                'MAX_WORKERS=12',
                'BATCH_SIZE=34',
            ]
        )
        + '\n'
    )


def write_registry(path: Path) -> None:
    path.write_text(
        json.dumps(
            {
                'datasets': [
                    {
                        'dataset_id': 'alpha',
                        'label': 'Alpha',
                        'root_path': 'data/root',
                        'db_path': 'data/db.sqlite',
                        'default_start_date': '2025-03-01',
                        'source_ids': ['r2', 'r1'],
                    }
                ]
            }
        )
    )


def test_initialize_runtime_loads_dataset_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    common = load_common()
    env_path = tmp_path / '.env'
    datasets_path = tmp_path / 'datasets.json'
    write_registry(datasets_path)
    write_env(env_path, datasets_path)

    monkeypatch.delenv('DATASETS_CONFIG_PATH', raising=False)
    monkeypatch.delenv('DEFAULT_DATASET', raising=False)
    monkeypatch.delenv('NETFLOW_DATASET', raising=False)

    common.initialize_runtime(str(env_path))

    assert common.DEFAULT_DATASET == 'alpha'
    assert common.ACTIVE_DATASET['dataset_id'] == 'alpha'
    assert common.NETFLOW_DATA_PATH.endswith('data/root')
    assert common.DATABASE_PATH.endswith('data/db.sqlite')
    assert common.AVAILABLE_ROUTERS == ['r2', 'r1']
    assert common.MAX_WORKERS == 12
    assert common.BATCH_SIZE == 34
    assert common.DATA_START_DATE == datetime(2025, 3, 1)


def test_load_dataset_registry_rejects_duplicate_ids(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    common = load_common()
    datasets_path = tmp_path / 'datasets.json'
    datasets_path.write_text(
        json.dumps(
            [
                {'dataset_id': 'dup', 'root_path': 'a', 'db_path': 'a.sqlite'},
                {'dataset_id': 'dup', 'root_path': 'b', 'db_path': 'b.sqlite'},
            ]
        )
    )
    monkeypatch.setenv('DATASETS_CONFIG_PATH', str(datasets_path))

    with pytest.raises(common.ConfigurationError, match="Duplicate dataset_id 'dup'"):
        common.load_dataset_registry()


def test_get_dataset_start_date_rejects_bad_format(monkeypatch: pytest.MonkeyPatch) -> None:
    common = load_common()
    monkeypatch.setattr(
        common,
        'get_dataset_config',
        lambda dataset_id=None: {'dataset_id': 'alpha', 'default_start_date': '03/01/2025'},
    )

    with pytest.raises(common.ConfigurationError, match='Expected YYYY-MM-DD'):
        common.get_dataset_start_date()
