"""Atomic-ish publish helpers for extracted NetFlow windows."""

from __future__ import annotations

import os
import shutil
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .sqlite import sqlite_artifact_paths


PublishRecord = tuple[Path, Path | None]


def remove_path(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def path_exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def copy_path(source: Path, target: Path) -> None:
    if source.is_dir():
        shutil.copytree(source, target)
    else:
        shutil.copy2(source, target)


def prepare_publish_source(source: Path, target: Path, work_dir: Path) -> tuple[Path, Path | None]:
    target.parent.mkdir(parents=True, exist_ok=True)
    if source.stat().st_dev == target.parent.stat().st_dev:
        return source, None

    staging_path = target.parent / f".incoming-{target.name}-{work_dir.name}"
    remove_path(staging_path)
    try:
        copy_path(source, staging_path)
    except Exception:
        remove_path(staging_path)
        raise
    return staging_path, staging_path


def rollback_publish(installed: list[PublishRecord]) -> None:
    for target, backup_path in reversed(installed):
        remove_path(target)
        if backup_path is not None:
            os.replace(backup_path, target)


def backup_target(target: Path, backup_root: Path) -> PublishRecord | None:
    backup_path = target.parent / f".previous-{target.name}-{backup_root.name}"
    target.parent.mkdir(parents=True, exist_ok=True)
    remove_path(backup_path)
    if not path_exists(target):
        return None
    os.replace(target, backup_path)
    return target, backup_path


def publish_path(source: Path, target: Path, backup_root: Path) -> tuple[Path, Path | None, Path | None]:
    publish_source, staging_path = prepare_publish_source(source, target, backup_root)
    backup_record = backup_target(target, backup_root)
    backup_path = None if backup_record is None else backup_record[1]
    try:
        os.replace(publish_source, target)
    except Exception:
        if backup_path is not None:
            os.replace(backup_path, target)
        if staging_path is not None:
            remove_path(staging_path)
        raise
    return target, backup_path, staging_path


def publish_sqlite_artifacts(source: Path, target: Path, backup_root: Path) -> tuple[list[PublishRecord], list[Path]]:
    if not path_exists(source):
        raise FileNotFoundError(source)

    installed: list[PublishRecord] = []
    staging_paths: list[Path] = []
    prepared_sources: list[tuple[Path, Path, Path | None]] = []

    try:
        for source_artifact, target_artifact in zip(
            sqlite_artifact_paths(source),
            sqlite_artifact_paths(target),
            strict=True,
        ):
            if path_exists(source_artifact):
                publish_source, staging_path = prepare_publish_source(
                    source_artifact,
                    target_artifact,
                    backup_root,
                )
                prepared_sources.append((publish_source, target_artifact, staging_path))
                if staging_path is not None:
                    staging_paths.append(staging_path)

            backup_record = backup_target(target_artifact, backup_root)
            if backup_record is not None:
                installed.append(backup_record)

        for publish_source, target_artifact, _ in prepared_sources:
            os.replace(publish_source, target_artifact)
            if all(target_artifact != installed_target for installed_target, _ in installed):
                installed.append((target_artifact, None))
    except Exception:
        rollback_publish(installed)
        for staging_path in staging_paths:
            remove_path(staging_path)
        raise

    return installed, staging_paths


def replace_path(source: Path, target: Path, backup_root: Path) -> tuple[Path, Path | None, Path | None] | None:
    return publish_path(source, target, backup_root)


def publish_outputs(
    *,
    temp_sqlite_path: Path | None,
    sqlite_output_path: Path | None,
    temp_parquet_dir: Path | None,
    parquet_dir: Path | None,
    temp_manifest_path: Path,
    manifest_path: Path,
    work_dir: Path,
) -> None:
    publish_outputs_with_helpers(
        temp_sqlite_path=temp_sqlite_path,
        sqlite_output_path=sqlite_output_path,
        temp_parquet_dir=temp_parquet_dir,
        parquet_dir=parquet_dir,
        temp_manifest_path=temp_manifest_path,
        manifest_path=manifest_path,
        work_dir=work_dir,
        publish_sqlite_artifacts_func=publish_sqlite_artifacts,
        replace_path_func=replace_path,
        publish_path_func=publish_path,
        rollback_publish_func=rollback_publish,
        remove_path_func=remove_path,
    )


def publish_outputs_with_helpers(
    *,
    temp_sqlite_path: Path | None,
    sqlite_output_path: Path | None,
    temp_parquet_dir: Path | None,
    parquet_dir: Path | None,
    temp_manifest_path: Path,
    manifest_path: Path,
    work_dir: Path,
    publish_sqlite_artifacts_func: Callable[[Path, Path, Path], tuple[list[PublishRecord], list[Path]]],
    replace_path_func: Callable[[Path, Path, Path], Any],
    publish_path_func: Callable[[Path, Path, Path], tuple[Path, Path | None, Path | None]],
    rollback_publish_func: Callable[[list[PublishRecord]], None],
    remove_path_func: Callable[[Path], None],
) -> None:
    installed: list[PublishRecord] = []
    staging_paths: list[Path] = []
    backups: list[Path] = []
    try:
        if temp_sqlite_path is not None and sqlite_output_path is not None:
            sqlite_installed, sqlite_staging_paths = publish_sqlite_artifacts_func(
                temp_sqlite_path,
                sqlite_output_path,
                work_dir,
            )
            installed.extend(sqlite_installed)
            backups.extend(backup_path for _, backup_path in sqlite_installed if backup_path is not None)
            staging_paths.extend(sqlite_staging_paths)

        if temp_parquet_dir is not None and parquet_dir is not None:
            result = replace_path_func(temp_parquet_dir, parquet_dir, work_dir)
            if result is not None:
                installed_target, backup_path, staging_path = result
                installed.append((installed_target, backup_path))
                if backup_path is not None:
                    backups.append(backup_path)
                if staging_path is not None:
                    staging_paths.append(staging_path)

        installed_target, backup_path, staging_path = publish_path_func(
            temp_manifest_path,
            manifest_path,
            work_dir,
        )
        installed.append((installed_target, backup_path))
        if backup_path is not None:
            backups.append(backup_path)
        if staging_path is not None:
            staging_paths.append(staging_path)
    except Exception:
        rollback_publish_func(installed)
        for staging_path in staging_paths:
            remove_path_func(staging_path)
        raise

    for backup_path in backups:
        remove_path_func(backup_path)
    for staging_path in staging_paths:
        remove_path_func(staging_path)
