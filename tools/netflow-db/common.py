"""
Shared utilities for NetFlow database processing modules.

Centralizes environment loading, dataset registry access, database helpers, and
path utilities.
"""

import json
import os
from pathlib import Path
from datetime import datetime
from typing import Any, Optional


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ENV_PATH = REPO_ROOT / '.env'
DEFAULT_DATASETS_PATH = REPO_ROOT / 'datasets.json'
DEFAULT_DATA_START_DATE = datetime(2025, 2, 1)


class ConfigurationError(RuntimeError):
    """Raised when required runtime configuration is missing or invalid."""


def load_env_file(env_path: Optional[str] = None) -> None:
    """
    Load environment variables from a dotenv-style file into os.environ.
    
    Reads the file at env_path (default repo-level '.env'), ignoring empty lines
    and lines starting with '#'. Each non-comment line containing '=' is split on
    the first '=' and the left/right parts are stripped and set as KEY=VALUE in
    os.environ.
    
    If the file does not exist, prints an error message and exits the process with
    status code 1.
    """
    env_file = DEFAULT_ENV_PATH if env_path is None else Path(env_path).expanduser()
    if env_file.exists():
        with open(env_file, 'r') as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    key, value = line.split('=', 1)
                    os.environ[key.strip()] = value.strip()
    else:
        raise ConfigurationError(
            f"Environment file '{env_file}' not found. Please copy .env.example to .env and configure your settings."
        )


def get_optional_env(key: str, default: str = '') -> str:
    """Get optional environment variable with a default value."""
    return os.environ.get(key, default)


def resolve_repo_path(path_value: str) -> Path:
    """Resolve a path relative to the repository root when needed."""
    path = Path(path_value).expanduser()
    if path.is_absolute():
        return path
    return (REPO_ROOT / path).resolve()


def load_dataset_registry() -> list[dict[str, Any]]:
    """Load and normalize the dataset registry."""
    config_path = Path(
        get_optional_env('DATASETS_CONFIG_PATH', str(DEFAULT_DATASETS_PATH))
    ).expanduser()

    if not config_path.exists():
        raise ConfigurationError(
            f"Dataset registry '{config_path}' not found. Configure DATASETS_CONFIG_PATH or create datasets.json."
        )

    with open(config_path, 'r') as f:
        raw = json.load(f)

    entries = raw.get('datasets') if isinstance(raw, dict) else raw
    if not isinstance(entries, list) or not entries:
        raise ConfigurationError(f"Dataset registry '{config_path}' is empty or invalid.")

    normalized: list[dict[str, Any]] = []
    seen_ids: set[str] = set()

    for entry in entries:
        if not isinstance(entry, dict):
            raise ConfigurationError(f"Invalid dataset entry in '{config_path}': {entry!r}")

        dataset_id = str(entry.get('dataset_id', '')).strip()
        root_path = str(entry.get('root_path', '')).strip()
        db_path = str(entry.get('db_path', '')).strip()

        if not dataset_id or not root_path or not db_path:
            raise ConfigurationError(f"Dataset entry missing required fields: {entry!r}")
        if dataset_id in seen_ids:
            raise ConfigurationError(f"Duplicate dataset_id '{dataset_id}' in '{config_path}'")
        seen_ids.add(dataset_id)

        source_ids = entry.get('source_ids')
        if source_ids is not None and not isinstance(source_ids, list):
            raise ConfigurationError(f"source_ids must be a list when provided for '{dataset_id}'")
        sources = normalize_dataset_sources(entry, dataset_id)

        normalized.append(
            {
                'dataset_id': dataset_id,
                'label': str(entry.get('label', dataset_id.replace('_', ' ').title())),
                'root_path': str(Path(root_path).expanduser()),
                'db_path': str(resolve_repo_path(db_path)),
                'default_start_date': str(entry.get('default_start_date', '')).strip(),
                'source_mode': str(entry.get('source_mode', 'subdirs')),
                'discovery_mode': str(entry.get('discovery_mode', 'static')),
                'sort_order': int(entry.get('sort_order') or 0),
                'source_ids': [str(source).strip() for source in (source_ids or []) if str(source).strip()],
                'sources': sources,
            }
        )

    return normalized


def normalize_dataset_sources(entry: dict[str, Any], dataset_id: str) -> list[dict[str, Any]]:
    """Normalize optional logical source definitions from datasets.json."""
    raw_sources = entry.get('sources')
    raw_source_ids = entry.get('source_ids')
    if raw_sources is None:
        return []
    if raw_source_ids is not None:
        raise ConfigurationError(f"Dataset '{dataset_id}' cannot define both sources and source_ids")
    if not isinstance(raw_sources, list) or not raw_sources:
        raise ConfigurationError(f"sources must be a non-empty list when provided for '{dataset_id}'")

    normalized = []
    seen_source_ids = set()
    for raw_source in raw_sources:
        if not isinstance(raw_source, dict):
            raise ConfigurationError(f"Invalid source entry for '{dataset_id}': {raw_source!r}")
        source_id = str(raw_source.get('source_id', '')).strip()
        members = raw_source.get('members')
        if not source_id:
            raise ConfigurationError(f"Source entry missing source_id for '{dataset_id}'")
        if source_id in seen_source_ids:
            raise ConfigurationError(f"Duplicate source_id '{source_id}' for dataset '{dataset_id}'")
        if not isinstance(members, list) or not members:
            raise ConfigurationError(f"Source '{source_id}' must define a non-empty members list")
        normalized_members = [str(member).strip() for member in members if str(member).strip()]
        if len(normalized_members) != len(members):
            raise ConfigurationError(f"Source '{source_id}' has an empty member id")
        if len(set(normalized_members)) != len(normalized_members):
            raise ConfigurationError(f"Source '{source_id}' contains duplicate members")
        seen_source_ids.add(source_id)
        normalized.append({'source_id': source_id, 'members': normalized_members})
    return normalized


def get_default_dataset_id() -> str:
    """Return the default dataset id."""
    configured = get_optional_env('DEFAULT_DATASET')
    if configured:
        return configured
    return DATASET_REGISTRY[0]['dataset_id']


def get_dataset_config(dataset_id: Optional[str] = None) -> dict[str, Any]:
    """Return the dataset config for the provided or active dataset id."""
    selected = dataset_id or get_optional_env('NETFLOW_DATASET', DEFAULT_DATASET)
    for config in DATASET_REGISTRY:
        if config['dataset_id'] == selected:
            return config

    available = ', '.join(config['dataset_id'] for config in DATASET_REGISTRY)
    raise ConfigurationError(f"Unknown dataset '{selected}'. Available datasets: {available}")


def list_dataset_sources(dataset_id: Optional[str] = None) -> list[str]:
    """Return the configured or discovered sources for a dataset."""
    config = get_dataset_config(dataset_id)
    configured_logical_sources = config.get('sources') or []
    if configured_logical_sources:
        return [source['source_id'] for source in configured_logical_sources]
    configured_sources = config.get('source_ids') or []
    if configured_sources:
        return configured_sources

    root_path = Path(config['root_path'])
    if not root_path.exists():
        return []

    if config.get('source_mode', 'subdirs') != 'subdirs':
        raise ValueError(f"Unsupported source_mode: {config.get('source_mode')}")

    return sorted(
        entry.name
        for entry in root_path.iterdir()
        if entry.is_dir()
    )


def get_dataset_root_path(dataset_id: Optional[str] = None) -> Path:
    """Return the root path for a dataset."""
    return Path(get_dataset_config(dataset_id)['root_path'])


def get_dataset_db_path(dataset_id: Optional[str] = None) -> Path:
    """Return the SQLite path for a dataset."""
    return Path(get_dataset_config(dataset_id)['db_path'])


def get_dataset_start_date(dataset_id: Optional[str] = None) -> datetime:
    """Return the processing start date for a dataset."""
    config = get_dataset_config(dataset_id)
    raw_value = str(config.get('default_start_date', '')).strip()
    if not raw_value:
        return DEFAULT_DATA_START_DATE

    try:
        return datetime.strptime(raw_value, '%Y-%m-%d')
    except ValueError as error:
        raise ConfigurationError(
            f"Invalid default_start_date '{raw_value}' for dataset '{config['dataset_id']}'. Expected YYYY-MM-DD."
        ) from error


DATASET_REGISTRY: list[dict[str, Any]] = []
DEFAULT_DATASET = ''
ACTIVE_DATASET: dict[str, Any] = {}
NETFLOW_DATA_PATH = ''
AVAILABLE_ROUTERS: list[str] = []
DATABASE_PATH = ''
MAX_WORKERS = 8
BATCH_SIZE = 50
DATA_START_DATE = DEFAULT_DATA_START_DATE


def initialize_runtime(env_path: Optional[str] = None) -> None:
    """Load environment and derive module-level runtime configuration."""
    global DATASET_REGISTRY
    global DEFAULT_DATASET
    global ACTIVE_DATASET
    global NETFLOW_DATA_PATH
    global AVAILABLE_ROUTERS
    global DATABASE_PATH
    global MAX_WORKERS
    global BATCH_SIZE
    global DATA_START_DATE

    load_env_file(env_path)
    dataset_registry = load_dataset_registry()
    default_dataset = get_default_dataset_id()
    selected_dataset = get_optional_env('NETFLOW_DATASET', default_dataset)
    active_dataset = next(
        (config for config in dataset_registry if config['dataset_id'] == selected_dataset),
        None,
    )
    if active_dataset is None:
        available = ', '.join(config['dataset_id'] for config in dataset_registry)
        raise ConfigurationError(f"Unknown dataset '{selected_dataset}'. Available datasets: {available}")

    netflow_data_path = str(Path(active_dataset['root_path']))
    configured_sources = active_dataset.get('source_ids') or []
    if configured_sources:
        available_routers = configured_sources
    else:
        root_path = Path(active_dataset['root_path'])
        if not root_path.exists():
            available_routers = []
        elif active_dataset.get('source_mode', 'subdirs') != 'subdirs':
            raise ValueError(f"Unsupported source_mode: {active_dataset.get('source_mode')}")
        else:
            available_routers = sorted(entry.name for entry in root_path.iterdir() if entry.is_dir())

    database_path = str(Path(active_dataset['db_path']))
    max_workers = int(get_optional_env('MAX_WORKERS', '8'))
    batch_size = int(get_optional_env('BATCH_SIZE', '50'))
    raw_start_date = str(active_dataset.get('default_start_date', '')).strip()
    if raw_start_date:
        try:
            data_start_date = datetime.strptime(raw_start_date, '%Y-%m-%d')
        except ValueError as error:
            raise ConfigurationError(
                f"Invalid default_start_date '{raw_start_date}' for dataset '{active_dataset['dataset_id']}'. Expected YYYY-MM-DD."
            ) from error
    else:
        data_start_date = DEFAULT_DATA_START_DATE

    DATASET_REGISTRY = dataset_registry
    DEFAULT_DATASET = default_dataset
    ACTIVE_DATASET = active_dataset
    NETFLOW_DATA_PATH = netflow_data_path
    AVAILABLE_ROUTERS = available_routers
    DATABASE_PATH = database_path
    MAX_WORKERS = max_workers
    BATCH_SIZE = batch_size
    DATA_START_DATE = data_start_date


if get_optional_env('NETFLOW_DB_SKIP_AUTO_INIT') != '1':
    initialize_runtime()
