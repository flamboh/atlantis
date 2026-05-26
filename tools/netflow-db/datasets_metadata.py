"""Dataset metadata table shared by pipeline output and the web app."""

from __future__ import annotations

import sqlite3
from typing import Any


def init_datasets_table(conn: sqlite3.Connection) -> None:
    """Create the dataset metadata table if it does not exist."""
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS datasets (
            id TEXT PRIMARY KEY NOT NULL,
            label TEXT NOT NULL,
            default_start_date TEXT NOT NULL,
            source_mode TEXT NOT NULL DEFAULT 'static',
            discovery_mode TEXT NOT NULL DEFAULT 'static',
            sort_order INTEGER NOT NULL DEFAULT 0
        )
        """
    )
    conn.execute(
        """
        CREATE TABLE IF NOT EXISTS source_members (
            dataset_id TEXT NOT NULL,
            source_id TEXT NOT NULL,
            member_id TEXT NOT NULL,
            PRIMARY KEY (dataset_id, source_id, member_id)
        )
        """
    )
    conn.commit()


def upsert_dataset_metadata(conn: sqlite3.Connection, dataset: dict[str, Any]) -> None:
    """Insert or update one dataset metadata row."""
    init_datasets_table(conn)
    dataset_id = str(dataset['dataset_id']).strip()
    label = str(dataset.get('label') or dataset_id)
    default_start_date = str(dataset.get('default_start_date') or '2025-02-01').strip()
    source_mode = str(dataset.get('source_mode') or 'static').strip()
    discovery_mode = str(dataset.get('discovery_mode') or 'static').strip()
    sort_order = int(dataset.get('sort_order') or 0)

    conn.execute(
        """
        INSERT INTO datasets (
            id,
            label,
            default_start_date,
            source_mode,
            discovery_mode,
            sort_order
        ) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            label = excluded.label,
            default_start_date = excluded.default_start_date,
            source_mode = excluded.source_mode,
            discovery_mode = excluded.discovery_mode,
            sort_order = excluded.sort_order
        """,
        (dataset_id, label, default_start_date, source_mode, discovery_mode, sort_order),
    )
    upsert_source_members(conn, dataset_id, dataset.get('sources') or [])
    conn.commit()


def upsert_source_members(
    conn: sqlite3.Connection,
    dataset_id: str,
    sources: list[dict[str, Any]],
) -> None:
    """Replace logical source membership metadata for one dataset."""
    conn.execute('DELETE FROM source_members WHERE dataset_id = ?', (dataset_id,))
    rows = [
        (dataset_id, str(source['source_id']).strip(), str(member).strip())
        for source in sources
        for member in source.get('members', [])
        if str(source.get('source_id', '')).strip() and str(member).strip()
    ]
    if rows:
        conn.executemany(
            """
            INSERT OR REPLACE INTO source_members (dataset_id, source_id, member_id)
            VALUES (?, ?, ?)
            """,
            rows,
        )
